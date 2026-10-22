//! The script boundary owner: the server-owned side of the boundary, the host
//! endpoint, the pair constructor, and the route authority behind them - which
//! package generation owns a route, the table that holds those registrations, the
//! source fence a decision is admitted under, and the command owner a route
//! resolves to. Moved out of `lib.rs` by CP-REFACTOR-040 so the route types stay
//! private inside the single owner that uses them.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use arc_swap::ArcSwap;

use crate::command_permissions::ScriptCommandPermissions;
use crate::manifest::ValidatedScriptPluginManifest;
use crate::{MAX_PLAYER_COMMAND_ROOTS, MAX_PLUGIN_PAYLOAD_CHANNELS, precommit};

/// Error returned when active plugin routes cannot be registered.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScriptRouteRegistrationError {
    RootConflict {
        root: String,
        owner_plugin_id: String,
    },
    ChannelConflict {
        channel: String,
        owner_plugin_id: String,
    },
    RootLimitExceeded {
        limit: usize,
        requested: usize,
    },
    ChannelLimitExceeded {
        limit: usize,
        requested: usize,
    },
    AuthorityPoisoned,
    DuplicatePermissionProvider {
        plugin_id: String,
    },
}

#[derive(Debug, Clone)]
struct PluginRouteAuthority {
    routes: Arc<ArcSwap<PluginRouteTable>>,
    /// The published cross-plugin command-permission table.
    ///
    /// It is shared by every clone of this authority because command admission
    /// reads it on the tick thread while the provider publishes from the host
    /// thread.
    permissions: Arc<ScriptCommandPermissions>,
    writer: Arc<StdMutex<()>>,
    disabled: Arc<AtomicBool>,
    /// The next live-registration generation this authority hands out.
    ///
    /// It is monotonic for the authority's whole life rather than per table, so a
    /// generation is never reused by a later table: a fence captured under one
    /// registration cannot validate against a different one that happened to be
    /// built into a fresh table by a reload.
    next_registration: Arc<AtomicU64>,
}

impl PluginRouteAuthority {
    fn new() -> Self {
        Self {
            routes: Arc::new(ArcSwap::from_pointee(PluginRouteTable::default())),
            permissions: Arc::default(),
            writer: Arc::default(),
            disabled: Arc::default(),
            next_registration: Arc::new(AtomicU64::new(1)),
        }
    }
}

#[derive(Debug, Clone, Default)]
struct PluginRouteTable {
    commands: BTreeMap<String, PluginCommandOwner>,
    payload_channels: BTreeMap<String, String>,
    /// The live registration of each registered package.
    ///
    /// A route table entry says what a package owns; this says *since when* and
    /// *with which grants*, which is what a decision admitted under that
    /// registration is later checked against.
    registrations: BTreeMap<String, PluginRegistration>,
}

/// One package's live registration generation in the boundary's authority.
#[derive(Debug, Clone)]
struct PluginRegistration {
    generation: u64,
    subscriptions: Arc<std::collections::BTreeSet<String>>,
}

/// One package's registration, as a decision was admitted under it.
///
/// Native command admission already checked the concrete operation and grant that
/// produced the request. This fence retains that exact registration generation,
/// rather than attempting to infer a capability from the hook it later reaches.
#[derive(Debug)]
struct PluginSourceFence {
    routes: PluginRouteAuthority,
    plugin_id: String,
    generation: u64,
}

impl precommit::SourceRegistration for PluginSourceFence {
    fn is_live(&self) -> bool {
        self.routes
            .registration_is_live(&self.plugin_id, self.generation)
    }
}

#[derive(Debug, Clone)]
struct PluginCommandOwner {
    plugin_id: String,
    operator_only: bool,
    /// The permission node a provider must grant before this root is admitted.
    permission: Option<String>,
}

impl PluginRouteAuthority {
    fn snapshot(&self) -> arc_swap::Guard<Arc<PluginRouteTable>> {
        if self.writer.is_poisoned() {
            self.disable();
        }
        self.routes.load()
    }

    /// Test-only: panic while serializing a publication, never a reader.
    #[cfg(test)]
    fn with_routes_write_for_test<R>(&self, f: impl FnOnce(&mut PluginRouteTable) -> R) -> R {
        let _guard = self.writer.lock().expect("route writer lock is free");
        let mut table = (**self.routes.load()).clone();
        f(&mut table)
    }

    fn roots(&self, operator_only: bool) -> Vec<String> {
        if self.disabled.load(Ordering::Acquire) {
            return Vec::new();
        }
        self.snapshot()
            .commands
            .iter()
            .filter(|(_, owner)| owner.operator_only == operator_only)
            .map(|(root, _)| root.clone())
            .collect()
    }

    fn has_channel(&self, channel: &str) -> bool {
        if self.disabled.load(Ordering::Acquire) {
            return false;
        }
        self.snapshot().payload_channels.contains_key(channel)
    }

    fn owner(&self, root: &str) -> Option<PluginCommandOwner> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        self.snapshot().commands.get(root).cloned()
    }

    fn channel_owner(&self, channel: &str) -> Option<String> {
        if self.disabled.load(Ordering::Acquire) {
            return None;
        }
        self.snapshot().payload_channels.get(channel).cloned()
    }

    fn register(
        &self,
        manifest: &ValidatedScriptPluginManifest,
    ) -> Result<(), ScriptRouteRegistrationError> {
        if self.disabled.load(Ordering::Acquire) {
            return Err(ScriptRouteRegistrationError::AuthorityPoisoned);
        }
        let _writer = self.lock_writer()?;
        if manifest.is_permission_provider()
            && self
                .permissions
                .provider()
                .is_some_and(|provider| provider != manifest.plugin_id())
        {
            return Err(ScriptRouteRegistrationError::DuplicatePermissionProvider {
                plugin_id: manifest.plugin_id().to_owned(),
            });
        }
        let mut routes = (**self.routes.load()).clone();
        let channels = manifest.custom_payload_channels();
        let requested_roots = manifest
            .player_command_roots()
            .iter()
            .chain(manifest.operator_command_roots())
            .collect::<Vec<_>>();
        let requested_roots = routes.commands.len().saturating_add(requested_roots.len());
        if requested_roots > MAX_PLAYER_COMMAND_ROOTS {
            return Err(ScriptRouteRegistrationError::RootLimitExceeded {
                limit: MAX_PLAYER_COMMAND_ROOTS,
                requested: requested_roots,
            });
        }
        let requested_channels = routes.payload_channels.len().saturating_add(channels.len());
        if requested_channels > MAX_PLUGIN_PAYLOAD_CHANNELS {
            return Err(ScriptRouteRegistrationError::ChannelLimitExceeded {
                limit: MAX_PLUGIN_PAYLOAD_CHANNELS,
                requested: requested_channels,
            });
        }
        if let Some((root, owner_plugin_id)) = manifest
            .player_command_roots()
            .iter()
            .chain(manifest.operator_command_roots())
            .find_map(|root| {
                routes
                    .commands
                    .get(root)
                    .map(|owner| (root, owner.plugin_id.clone()))
            })
        {
            return Err(ScriptRouteRegistrationError::RootConflict {
                root: root.clone(),
                owner_plugin_id,
            });
        }
        if let Some((channel, owner_plugin_id)) = channels.iter().find_map(|channel| {
            routes
                .payload_channels
                .get(*channel)
                .map(|owner| (channel, owner.clone()))
        }) {
            return Err(ScriptRouteRegistrationError::ChannelConflict {
                channel: (*channel).to_owned(),
                owner_plugin_id,
            });
        }
        for root in manifest.player_command_roots() {
            routes.commands.insert(
                root.clone(),
                PluginCommandOwner {
                    plugin_id: manifest.plugin_id().to_owned(),
                    operator_only: false,
                    permission: manifest.command_permissions().get(root).cloned(),
                },
            );
        }
        for root in manifest.operator_command_roots() {
            routes.commands.insert(
                root.clone(),
                PluginCommandOwner {
                    plugin_id: manifest.plugin_id().to_owned(),
                    operator_only: true,
                    permission: manifest.command_permissions().get(root).cloned(),
                },
            );
        }
        if manifest.is_permission_provider() {
            self.permissions.set_provider(Some(manifest.plugin_id()));
        }
        for channel in channels {
            routes
                .payload_channels
                .insert(channel.to_owned(), manifest.plugin_id().to_owned());
        }
        // Every registered package gets a live registration, whether or not it
        // owns a route: a package's own admitted request is authority this
        // boundary granted, and this is where its generation is recorded.
        routes.registrations.insert(
            manifest.plugin_id().to_owned(),
            PluginRegistration {
                generation: self.next_generation(),
                subscriptions: Arc::new(
                    manifest
                        .event_subscriptions()
                        .iter()
                        .map(|subscription| subscription.event_name().to_owned())
                        .collect(),
                ),
            },
        );
        self.routes.store(Arc::new(routes));
        Ok(())
    }

    fn next_generation(&self) -> u64 {
        self.next_registration.fetch_add(1, Ordering::AcqRel)
    }

    /// One package's live registration, as this boundary currently holds it.
    ///
    /// Event admission can be closed while the host drains inputs it already
    /// accepted. Those inputs retain their captured registrations; only a route
    /// replacement, unregistration, or poisoned authority makes them stale.
    fn registration(&self, plugin_id: &str) -> Option<PluginRegistration> {
        self.snapshot().registrations.get(plugin_id).cloned()
    }

    /// Whether the registration a decision was admitted under is still live.
    ///
    /// Its generation changes for every replacement, including a changed grant;
    /// operation-specific admission was checked before this question was built.
    fn registration_is_live(&self, plugin_id: &str, generation: u64) -> bool {
        self.registration(plugin_id)
            .is_some_and(|registration| registration.generation == generation)
    }

    fn unregister(&self, plugin_id: &str) {
        if self.disabled.load(Ordering::Acquire) {
            return;
        }
        let Ok(_writer) = self.lock_writer() else {
            return;
        };
        let mut routes = (**self.routes.load()).clone();
        routes
            .commands
            .retain(|_, owner| owner.plugin_id != plugin_id);
        routes
            .payload_channels
            .retain(|_, owner| owner.as_str() != plugin_id);
        routes.registrations.remove(plugin_id);
        if self.permissions.provider().as_deref() == Some(plugin_id) {
            self.permissions.set_provider(None);
        }
        self.routes.store(Arc::new(routes));
    }

    /// Replace every route with one candidate table, or refuse the whole table.
    ///
    /// Conflicts and limits are checked against a fully built replacement first,
    /// so a refused reload never leaves the boundary with half a route table.
    fn replace_all(
        &self,
        manifests: &[ValidatedScriptPluginManifest],
    ) -> Result<(), ScriptRouteRegistrationError> {
        if self.disabled.load(Ordering::Acquire) {
            return Err(ScriptRouteRegistrationError::AuthorityPoisoned);
        }
        let mut replacement = PluginRouteTable::default();
        let requested_roots = manifests.iter().fold(0_usize, |count, manifest| {
            count
                .saturating_add(manifest.player_command_roots().len())
                .saturating_add(manifest.operator_command_roots().len())
        });
        if requested_roots > MAX_PLAYER_COMMAND_ROOTS {
            return Err(ScriptRouteRegistrationError::RootLimitExceeded {
                limit: MAX_PLAYER_COMMAND_ROOTS,
                requested: requested_roots,
            });
        }
        let requested_channels = manifests.iter().fold(0_usize, |count, manifest| {
            count.saturating_add(manifest.custom_payload_channels().len())
        });
        if requested_channels > MAX_PLUGIN_PAYLOAD_CHANNELS {
            return Err(ScriptRouteRegistrationError::ChannelLimitExceeded {
                limit: MAX_PLUGIN_PAYLOAD_CHANNELS,
                requested: requested_channels,
            });
        }
        if let Some(second) = manifests
            .iter()
            .filter(|manifest| manifest.is_permission_provider())
            .nth(1)
        {
            return Err(ScriptRouteRegistrationError::DuplicatePermissionProvider {
                plugin_id: second.plugin_id().to_owned(),
            });
        }
        for manifest in manifests {
            for (root, operator_only) in manifest
                .player_command_roots()
                .iter()
                .map(|root| (root, false))
                .chain(
                    manifest
                        .operator_command_roots()
                        .iter()
                        .map(|root| (root, true)),
                )
            {
                if let Some(owner) = replacement.commands.insert(
                    root.clone(),
                    PluginCommandOwner {
                        plugin_id: manifest.plugin_id().to_owned(),
                        operator_only,
                        permission: manifest.command_permissions().get(root).cloned(),
                    },
                ) {
                    return Err(ScriptRouteRegistrationError::RootConflict {
                        root: root.clone(),
                        owner_plugin_id: owner.plugin_id,
                    });
                }
            }
            for channel in manifest.custom_payload_channels() {
                if let Some(owner) = replacement
                    .payload_channels
                    .insert(channel.to_owned(), manifest.plugin_id().to_owned())
                {
                    return Err(ScriptRouteRegistrationError::ChannelConflict {
                        channel: channel.to_owned(),
                        owner_plugin_id: owner,
                    });
                }
            }
            // A committed reload replaces every registration: a package that
            // survives it gets a new generation, so a decision admitted under the
            // previous deployment can never be committed under this one.
            replacement.registrations.insert(
                manifest.plugin_id().to_owned(),
                PluginRegistration {
                    generation: self.next_generation(),
                    subscriptions: Arc::new(
                        manifest
                            .event_subscriptions()
                            .iter()
                            .map(|subscription| subscription.event_name().to_owned())
                            .collect(),
                    ),
                },
            );
        }
        let _writer = self.lock_writer()?;
        self.routes.store(Arc::new(replacement));
        // A replacement is the deployment's whole authority: the provider slot
        // follows the new manifest set instead of a package that is gone.
        let provider = manifests
            .iter()
            .find(|manifest| manifest.is_permission_provider())
            .map(|manifest| manifest.plugin_id());
        self.permissions.set_provider(provider);
        Ok(())
    }

    fn lock_writer(&self) -> Result<std::sync::MutexGuard<'_, ()>, ScriptRouteRegistrationError> {
        match self.writer.lock() {
            Ok(writer) if !self.disabled.load(Ordering::Acquire) => Ok(writer),
            Ok(_) => Err(ScriptRouteRegistrationError::AuthorityPoisoned),
            Err(_) => {
                self.disable();
                Err(ScriptRouteRegistrationError::AuthorityPoisoned)
            }
        }
    }

    /// Close new route admission without invalidating events already in the FIFO.
    fn clear(&self) {
        self.disabled.store(true, Ordering::Release);
    }

    fn disable(&self) {
        self.disabled.store(true, Ordering::Release);
        self.clear_poisoned();
    }

    fn clear_poisoned(&self) {
        self.routes.store(Arc::new(PluginRouteTable::default()));
    }
}

/// Server-owned side of the script boundary.
#[derive(Debug, Clone)]
pub struct ScriptBoundary {
    event_admission: Arc<ScriptEventAdmission>,
    command_rx: Arc<Mutex<mpsc::Receiver<ScriptCommand>>>,
    plugin_routes: PluginRouteAuthority,
    host_admissions: Arc<HostAdmissionLedger>,
    deployed_packages: Arc<[PluginPackage]>,
    observations: Arc<StdMutex<ObservationQueues>>,
    /// The operator's hook registrations and the questions admitted under them.
    ///
    /// It is shared by every clone of this boundary rather than copied per clone,
    /// because a game owner holds its own clone: the roster a question was admitted
    /// under has to be the same roster the approval is checked against.
    precommit: Arc<precommit::PrecommitAuthority>,
}

#[derive(Debug)]
struct ScriptEventAdmission {
    closed: AtomicBool,
    sender: StdMutex<Option<mpsc::Sender<ScriptHostInput>>>,
    weak_sender: mpsc::WeakSender<ScriptHostInput>,
    event_capacity: usize,
    coalesced_server_tick: Arc<StdMutex<CoalescedServerTick>>,
}

#[derive(Debug, Default)]
struct CoalescedServerTick {
    pending: Option<u64>,
    highest_seen: Option<u64>,
}

const MAX_OBSERVATIONS_PER_PLUGIN: usize = 256;

#[derive(Debug, Default)]
struct ObservationQueues {
    plugins: BTreeMap<String, PluginObservations>,
    last_delivered: Option<String>,
}

#[derive(Debug)]
struct PluginObservations {
    generation: u64,
    events: std::collections::VecDeque<(u64, ScriptHostEvent)>,
    dropped: u64,
}

impl ScriptEventAdmission {
    fn sender(&self) -> Option<mpsc::Sender<ScriptHostInput>> {
        if self.closed.load(Ordering::Acquire) {
            return None;
        }
        let sender = self.weak_sender.upgrade()?;
        (!self.closed.load(Ordering::Acquire)).then_some(sender)
    }

    fn close(&self) {
        self.closed.store(true, Ordering::Release);
        match self.sender.lock() {
            Ok(mut sender) => {
                sender.take();
            }
            Err(poisoned) => {
                poisoned.into_inner().take();
            }
        }
    }
}

impl ScriptBoundary {
    /// Poison the admission ledger the way a panic under its lock would, for the
    /// regression that proves a poisoned ledger refuses to issue.
    #[cfg(test)]
    pub(super) fn poison_host_admissions_for_test(&self) {
        let ledger = Arc::clone(&self.host_admissions);
        let poisoned = std::thread::spawn(move || {
            let _guard = ledger.pending.lock().unwrap();
            panic!("poison host admission ledger");
        });
        assert!(poisoned.join().is_err());
    }

    /// Pop the next command the boundary queued, for the queue contracts.
    #[cfg(test)]
    pub(super) fn try_recv_command_for_test(&self) -> Option<ScriptCommand> {
        let mut receiver = self
            .command_rx
            .try_lock()
            .expect("the command receiver lock is free");
        match receiver.try_recv() {
            Ok(command) => Some(command),
            Err(mpsc::error::TryRecvError::Empty) => None,
            Err(mpsc::error::TryRecvError::Disconnected) => {
                panic!("the command receiver is disconnected")
            }
        }
    }

    /// The live precommit-hook generation, for the roster contracts.
    #[cfg(test)]
    pub(super) fn precommit_generation_for_test(&self) -> u64 {
        self.precommit.generation()
    }

    /// Handle the trusted host queues its own control input through.
    ///
    /// The handle reaches the same bounded mailbox as events and cannot outlive
    /// the boundary's open host side; [`ScriptHostInputSender`] explains what a
    /// host may put in it and why it is narrow.
    #[must_use]
    pub fn host_input_sender(&self) -> ScriptHostInputSender {
        ScriptHostInputSender {
            sender: self.event_admission.weak_sender.clone(),
        }
    }

    fn admit_event(&self, event: ScriptEvent) -> ScriptHostEvent {
        let target_generation = match event.target_plugin_id() {
            Some(plugin_id) => {
                EventTargetGeneration::Targeted(event.origin_generation.or_else(|| {
                    self.plugin_routes
                        .registration(plugin_id)
                        .map(|registration| registration.generation)
                }))
            }
            None => EventTargetGeneration::Untargeted,
        };
        ScriptHostEvent {
            event,
            target_generation,
        }
    }

    /// Enqueue an immutable event without blocking a server task.
    pub fn try_enqueue_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        event_tx
            .try_send(ScriptHostInput::Event(self.admit_event(event)))
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ScriptQueueError::Full,
                mpsc::error::TrySendError::Closed(_) => ScriptQueueError::Closed,
            })
    }

    /// Best-effort observations have a separate, 256-entry queue per registered
    /// subscriber. A full queue drops the newest observation for that plugin
    /// only. The host round-robins these queues, independently of required input.
    /// An observation becomes visible only after a server-tick WAL ACK reaches
    /// the host; callers must publish that clock through the ordinary event FIFO.
    pub fn enqueue_observation(&self, event: ScriptEvent) {
        let routes = self.plugin_routes.snapshot();
        let mut targeted_events = Vec::new();
        for (id, registration) in &routes.registrations {
            if event.target_plugin_id().is_some_and(|target| target != id)
                || (event.target_plugin_id().is_none()
                    && !registration.subscriptions.contains(event.event_name()))
            {
                continue;
            }
            let mut targeted = event.clone();
            targeted.target_plugin_id = Some(id.clone());
            targeted_events.push((
                id.clone(),
                registration.generation,
                ScriptHostEvent {
                    event: targeted,
                    target_generation: EventTargetGeneration::Targeted(Some(
                        registration.generation,
                    )),
                },
            ));
        }
        let mut observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        // A reload can publish while the event copies are being prepared.
        // Never let an older snapshot evict a newer registration's queue.
        let current_routes = self.plugin_routes.snapshot();
        observations.plugins.retain(|id, queue| {
            current_routes
                .registrations
                .get(id)
                .is_some_and(|registration| registration.generation == queue.generation)
        });
        for (id, generation, targeted) in targeted_events {
            if current_routes
                .registrations
                .get(&id)
                .is_none_or(|registration| registration.generation != generation)
            {
                // Admission belongs to the captured registration, just like
                // queued host inputs; never rebind old events to a replacement.
                continue;
            }
            let queue = observations
                .plugins
                .entry(id)
                .or_insert_with(|| PluginObservations {
                    generation,
                    events: Default::default(),
                    dropped: 0,
                });
            if self.event_admission.closed.load(Ordering::Acquire)
                || queue.events.len() >= MAX_OBSERVATIONS_PER_PLUGIN
            {
                queue.dropped = queue.dropped.saturating_add(1);
                continue;
            }
            queue.events.push_back((u64::MAX, targeted));
        }
    }

    /// Bind pending observations to their completed native tick, before its WAL
    /// append. They stay invisible until that tick's acknowledged clock arrives.
    pub fn seal_observations(&self, tick: u64) {
        let mut observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
        for queue in observations.plugins.values_mut() {
            for (pending_tick, _) in queue.events.iter_mut().rev() {
                if *pending_tick != u64::MAX {
                    break;
                }
                *pending_tick = tick;
            }
        }
    }

    /// Number of observations shed for this plugin's current registration.
    /// Durable completion callbacks never contribute to this counter.
    pub fn observation_events_dropped(&self, plugin_id: &str) -> u64 {
        let Some(registration) = self.plugin_routes.registration(plugin_id) else {
            return 0;
        };
        self.observations
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .plugins
            .get(plugin_id)
            .filter(|queue| queue.generation == registration.generation)
            .map_or(0, |queue| queue.dropped)
    }

    /// Push the latest simulation tick without blocking or losing timer progress.
    ///
    /// When the normal event queue is full, newer ticks replace the one pending
    /// coalesced tick. The host drains ordinary queued events first, then observes
    /// the latest tick before blocking again.
    pub fn try_enqueue_latest_server_tick(&self, tick: u64) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        let input =
            ScriptHostInput::Event(ScriptHostEvent::untargeted(ScriptEvent::server_tick(tick)));
        let mut coalesced = match self.event_admission.coalesced_server_tick.lock() {
            Ok(coalesced) => coalesced,
            Err(poisoned) => poisoned.into_inner(),
        };
        if coalesced
            .highest_seen
            .is_some_and(|highest| tick <= highest)
        {
            return Ok(());
        }
        coalesced.highest_seen = Some(tick);
        // Keep publication atomic with the FIFO send: otherwise the consumer
        // can drain its last input before the pending tick becomes visible.
        match event_tx.try_send(input) {
            Ok(()) => {
                coalesced.pending = None;
                Ok(())
            }
            Err(mpsc::error::TrySendError::Full(_)) => {
                coalesced.pending = Some(tick);
                Ok(())
            }
            Err(mpsc::error::TrySendError::Closed(_)) => Err(ScriptQueueError::Closed),
        }
    }

    /// Deliver a required event, waiting for bounded host-queue capacity.
    ///
    /// Capacity and closure wake this future through the channel. This deliberately
    /// differs from lossy telemetry submitted through [`Self::try_enqueue_event`].
    pub async fn enqueue_required_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        let Some(event_tx) = self.event_admission.sender() else {
            return Err(ScriptQueueError::Closed);
        };
        event_tx
            .send(ScriptHostInput::Event(self.admit_event(event)))
            .await
            .map_err(|_| ScriptQueueError::Closed)
    }

    /// Deliver a targeted owner event through the required-delivery path.
    pub async fn enqueue_targeted_event(&self, event: ScriptEvent) -> Result<(), ScriptQueueError> {
        self.enqueue_required_event(event).await
    }

    /// Whether a plugin currently owns a live event route. Native continuations
    /// use this before changing state on the owner's behalf.
    #[must_use]
    pub fn plugin_is_active(&self, plugin_id: &str) -> bool {
        self.plugin_routes.registration(plugin_id).is_some()
    }

    /// Recover native event ownership only for an actually live host registration.
    ///
    /// Persisted menu/zone owners cannot invent a plugin or revive retired routes.
    pub fn plugin_target(&self, plugin_id: &str) -> Option<crate::ScriptPluginTarget> {
        self.plugin_is_active(plugin_id)
            .then(|| crate::ScriptPluginTarget::from_registration(plugin_id))
    }

    /// Stop accepting new host events while allowing already admitted events to drain.
    pub fn close_event_admission(&self) {
        self.event_admission.close();
        self.plugin_routes.clear();
    }

    /// Every deployed package the host discovered, with its manifest facts.
    #[must_use]
    pub fn deployed_packages(&self) -> &[PluginPackage] {
        &self.deployed_packages
    }

    /// Publish the deployed packages the host discovered for this deployment.
    ///
    /// Discovery belongs to the host, which already parsed every manifest; the
    /// boundary only carries the result to core's authored-data subsystems, so
    /// this replaces the catalog in one step instead of re-reading packages.
    pub fn set_deployed_packages(&mut self, packages: Vec<PluginPackage>) {
        self.deployed_packages = Arc::from(packages);
    }

    /// Publish the operator's pre-commit hook registrations for this deployment.
    ///
    /// The host calls this once, after discovery, with exactly the registrations
    /// each discovered package was authorized for: the roster an operator
    /// configured is therefore the roster a game owner's questions are admitted
    /// against, and no second list can disagree with it. A refused roster is left
    /// unpublished, so a malformed one never replaces the registrations a live
    /// chain is answering under.
    pub fn set_precommit_hooks(
        &self,
        hooks: Vec<precommit::HookRegistration>,
    ) -> Result<(), precommit::HookRosterError> {
        self.precommit.set_hooks(hooks)
    }

    /// Whether any package is registered for this hook.
    ///
    /// This is the zero-handler fast branch a game owner takes first: with no
    /// registration there is no question to ask, and the effect follows the direct
    /// native path it always followed - nothing is queued, nothing is allocated and
    /// no deadline is created.
    #[must_use]
    pub fn has_precommit_hooks(&self, kind: precommit::HookKind) -> bool {
        self.precommit.has(kind)
    }

    /// Ask the registered packages about one effect before it is committed.
    ///
    /// The question is queued nonblocking onto the same bounded FIFO the host's
    /// events and control input travel, under one absolute deadline created here
    /// that covers the queue wait and the whole handler chain. A boundary with no
    /// registration for this hook answers the direct path immediately.
    pub fn begin_precommit(
        &self,
        context: precommit::HookContext,
    ) -> Result<precommit::PendingDecision, precommit::HookFailure> {
        self.begin_precommit_with_deadline(context, precommit::deadline_from_now())
    }

    /// The same, under a deadline the caller owns.
    ///
    /// The value is a bound in absolute time rather than a duration, so a caller
    /// that has to fit the question inside its own budget can state it, and a test
    /// can inject a deadline it controls instead of sleeping towards one.
    pub fn begin_precommit_with_deadline(
        &self,
        context: precommit::HookContext,
        deadline: std::time::Instant,
    ) -> Result<precommit::PendingDecision, precommit::HookFailure> {
        let source = self.plugin_source_fence(&context)?;
        precommit::begin(&self.precommit, context, deadline, source, |request| {
            let Some(sender) = self.event_admission.sender() else {
                return Err(precommit::HookFailure::Unavailable);
            };
            sender
                .try_send(ScriptHostInput::Precommit(request))
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(_) => precommit::HookFailure::QueueFull,
                    mpsc::error::TrySendError::Closed(_) => precommit::HookFailure::Unavailable,
                })
        })
    }

    /// The live registration authority a package's own request came from.
    ///
    /// Native admission already checked the concrete operation and grant before it
    /// built the hook context. The hook boundary captures that registration's
    /// generation and re-checks it at commit, so withdrawal or replacement cannot
    /// authorize a decision after the source authority is gone without guessing a
    /// capability from `HookKind`.
    fn plugin_source_fence(
        &self,
        context: &precommit::HookContext,
    ) -> Result<Option<Arc<dyn precommit::SourceRegistration>>, precommit::HookFailure> {
        let Some(plugin_id) = context.plugin_source() else {
            return Ok(None);
        };
        let registration = self
            .plugin_routes
            .registration(plugin_id)
            .ok_or(precommit::HookFailure::Stale)?;
        Ok(Some(Arc::new(PluginSourceFence {
            routes: self.plugin_routes.clone(),
            plugin_id: plugin_id.to_owned(),
            generation: registration.generation,
        })))
    }

    /// Number of event inputs currently waiting at the host boundary.
    ///
    /// This is diagnostic state only: producers still use the same nonblocking
    /// admission and full queues continue to refuse new work.
    #[must_use]
    pub fn event_queue_depth(&self) -> usize {
        self.event_admission
            .sender()
            .map(|sender| {
                self.event_admission
                    .event_capacity
                    .saturating_sub(sender.capacity())
            })
            .unwrap_or(0)
    }

    /// Return a sorted snapshot of currently active plugin command roots.
    pub fn player_command_roots(&self) -> Vec<String> {
        self.plugin_routes.roots(false)
    }

    /// The command-permission table this boundary shares with its host.
    ///
    /// Handed to the deployed permission provider so its publication is the same
    /// authority command admission reads.
    #[must_use]
    pub fn command_permissions(&self) -> Arc<ScriptCommandPermissions> {
        Arc::clone(&self.plugin_routes.permissions)
    }

    /// Return a sorted snapshot of active operator-only plugin command roots.
    pub fn operator_command_roots(&self) -> Vec<String> {
        self.plugin_routes.roots(true)
    }

    /// Prepare a targeted command snapshot without publishing it to the guest.
    ///
    /// Owners with a commit FIFO retain this event until their earlier sealed
    /// observations are admitted. Validation does not consume event capacity;
    /// permission and DTO refusals retain their ordinary routing semantics.
    pub fn prepare_player_command_with_context(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        raw: &str,
    ) -> Result<crate::PreparedPlayerCommand, ScriptQueueError> {
        let Some((root, arguments)) = split_player_command(raw) else {
            return Ok(crate::PreparedPlayerCommand::NotOwned);
        };
        let Some(owner) = self.plugin_routes.owner(root) else {
            return Ok(crate::PreparedPlayerCommand::NotOwned);
        };
        if owner.operator_only && !context.operator() {
            return Ok(crate::PreparedPlayerCommand::PermissionDenied);
        }
        // A declared node is a requirement only while a permission provider is
        // deployed: without one nobody can answer, and refusing every gated root
        // would silently disable a package the operator did deploy.
        if let Some(node) = &owner.permission
            && !self.plugin_routes.permissions.allows(context.uuid(), node)
        {
            return Ok(crate::PreparedPlayerCommand::PermissionDenied);
        };
        let event = match ScriptEvent::try_player_command_with_context(
            owner.plugin_id,
            player_id,
            context,
            root,
            arguments,
        ) {
            Ok(event) => event,
            Err(error) => return Ok(crate::PreparedPlayerCommand::OwnedRejected { error }),
        };
        if self
            .event_admission
            .sender()
            .is_none_or(|sender| sender.is_closed())
        {
            self.plugin_routes.clear();
            return Err(ScriptQueueError::Closed);
        }
        Ok(crate::PreparedPlayerCommand::Event(event))
    }

    /// Prepare and immediately admit an event for owners without a commit FIFO.
    pub fn try_enqueue_player_command_with_context(
        &self,
        player_id: ScriptPlayerId,
        context: ScriptPlayerContext,
        raw: &str,
    ) -> Result<PlayerCommandAdmission, ScriptQueueError> {
        let event = match self.prepare_player_command_with_context(player_id, context, raw)? {
            crate::PreparedPlayerCommand::NotOwned => return Ok(PlayerCommandAdmission::NotOwned),
            crate::PreparedPlayerCommand::PermissionDenied => {
                return Ok(PlayerCommandAdmission::PermissionDenied);
            }
            crate::PreparedPlayerCommand::OwnedRejected { error } => {
                return Ok(PlayerCommandAdmission::OwnedRejected { error });
            }
            crate::PreparedPlayerCommand::Event(event) => event,
        };
        match self.try_enqueue_event(event) {
            Ok(()) => Ok(PlayerCommandAdmission::Enqueued),
            Err(error @ ScriptQueueError::Full) => Err(error),
            Err(error @ ScriptQueueError::Closed) => {
                self.plugin_routes.clear();
                Err(error)
            }
        }
    }

    /// Return whether one custom-payload channel is currently owned.
    /// This consults the active route owner without allocating.
    pub fn allows_custom_payload(&self, channel: &str) -> bool {
        self.plugin_routes.has_channel(channel)
    }

    /// Route one owned custom-payload body to the registered channel owner.
    /// Unknown channels and bodies above the fixed host bound report `false`
    /// before event retention. The owned payload moves into the event on success.
    pub fn try_enqueue_custom_payload(
        &self,
        player_id: ScriptPlayerId,
        phase: ScriptProtocolPhase,
        channel: &str,
        payload: Vec<u8>,
    ) -> Result<bool, ScriptQueueError> {
        if payload.len() > MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES {
            return Ok(false);
        }
        let Some(owner) = self.plugin_routes.channel_owner(channel) else {
            return Ok(false);
        };
        let event = match ScriptEvent::custom_payload(owner, player_id, phase, channel, payload) {
            Ok(event) => event,
            Err(_) => return Ok(false),
        };
        match self.try_enqueue_event(event) {
            Ok(()) => Ok(true),
            Err(error @ ScriptQueueError::Full) => Err(error),
            Err(error @ ScriptQueueError::Closed) => {
                self.plugin_routes.clear();
                Err(error)
            }
        }
    }

    /// Wait for the next command emitted by the script host.
    pub async fn recv_command(&self) -> Option<ScriptCommand> {
        self.command_rx.lock().await.recv().await
    }

    /// Receive an admitted host command without waiting on the tick owner.
    ///
    /// A competing receiver holding the queue, an empty queue and a closed
    /// queue all return `None`; no command is removed until this caller owns
    /// the receiver.
    pub fn try_recv_command(&self) -> Option<ScriptCommand> {
        self.command_rx.try_lock().ok()?.try_recv().ok()
    }

    /// Consume one exact host-issued admission ticket.
    pub fn accept_host_command(
        &self,
        command: ScriptCommand,
    ) -> Result<AdmittedScriptCommand, ScriptCommandAcceptanceError> {
        let ScriptCommand::HostAttached {
            provenance,
            request,
        } = command
        else {
            return Err(ScriptCommandAcceptanceError::NotHostAttached);
        };
        let current_generation = self
            .plugin_routes
            .registration(provenance.plugin_id())
            .map(|registration| registration.generation);
        self.host_admissions
            .accept(provenance, request, current_generation)
    }
}

/// Script-host side of the bounded boundary.
#[derive(Debug)]
pub struct ScriptHostEndpoint {
    event_rx: mpsc::Receiver<ScriptHostInput>,
    coalesced_server_tick: Arc<StdMutex<CoalescedServerTick>>,
    coalesced_tick_due: bool,
    highest_delivered_tick: Option<u64>,
    command_tx: mpsc::Sender<ScriptCommand>,
    plugin_routes: PluginRouteAuthority,
    host_admissions: Arc<HostAdmissionLedger>,
    observations: Arc<StdMutex<ObservationQueues>>,
    observation_turn: bool,
}

impl ScriptHostEndpoint {
    /// The command-permission table this host shares with its boundary.
    #[must_use]
    pub fn command_permissions(&self) -> Arc<ScriptCommandPermissions> {
        Arc::clone(&self.plugin_routes.permissions)
    }

    /// Poison the route authority the way a panic under its lock would.
    #[cfg(test)]
    pub(super) fn poison_plugin_routes_for_test(&self) {
        let routes = self.plugin_routes.clone();
        let poisoned = std::thread::spawn(move || {
            routes.with_routes_write_for_test(|_table| panic!("poison plugin-route authority"));
        });
        assert!(poisoned.join().is_err());
    }

    /// The live player-command roots the route authority still serves.
    #[cfg(test)]
    pub(super) fn plugin_route_roots_for_test(&self) -> Vec<String> {
        self.plugin_routes.roots(false)
    }

    /// Whether a root still resolves to an owning plugin.
    #[cfg(test)]
    pub(super) fn plugin_route_owner_for_test(&self, root: &str) -> bool {
        self.plugin_routes.owner(root).is_some()
    }

    /// How many host-attested requests are still awaiting a decision.
    #[cfg(test)]
    pub(super) fn host_admissions_pending_len_for_test(&self) -> usize {
        self.host_admissions.pending_len()
    }

    /// Wait asynchronously until an event arrives or the server side closes.
    ///
    /// Control input is not an event: this consumer drops such a payload unread,
    /// and the drop closes whatever response that payload carries. A runtime that
    /// owns the payload takes it through [`Self::recv_input_blocking`] instead.
    pub async fn recv_event(&mut self) -> Option<ScriptEvent> {
        loop {
            match self.recv_input().await? {
                ScriptHostInput::Event(event) => return Some(event.into_event()),
                ScriptHostInput::Reload(_) => {}
                ScriptHostInput::Precommit(_) => {}
            }
        }
    }

    /// Block the dedicated host thread until an event arrives or the server side closes.
    ///
    /// Event-only, exactly like [`Self::recv_event`]: control input is dropped
    /// unread rather than interpreted here.
    pub fn recv_event_blocking(&mut self) -> Option<ScriptEvent> {
        loop {
            match self.recv_input_blocking()? {
                ScriptHostInput::Event(event) => return Some(event.into_event()),
                ScriptHostInput::Reload(_) => {}
                ScriptHostInput::Precommit(_) => {}
            }
        }
    }

    /// Block the dedicated host thread until any input arrives or the server closes.
    ///
    /// The trusted host takes events and its own control input from this one FIFO,
    /// in order, so a queued reload never overtakes the events admitted before it.
    /// A targeted event whose admitted registration is no longer live is discarded
    /// before a component runtime can dispatch it.
    pub fn recv_input_blocking(&mut self) -> Option<ScriptHostInput> {
        loop {
            match self.try_recv_input() {
                Ok(input) => return Some(input),
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            let Some(input) = self.event_rx.blocking_recv() else {
                continue;
            };
            self.coalesced_tick_due = has_coalesced_server_tick(&self.coalesced_server_tick);
            if let Some(input) = self.accept_input(input) {
                return Some(input);
            }
        }
    }

    fn next_observation(&mut self) -> Option<ScriptHostInput> {
        let written_tick = self.highest_delivered_tick?;
        loop {
            let mut observations = self.observations.lock().unwrap_or_else(|e| e.into_inner());
            let ready = |queue: &&PluginObservations| {
                queue
                    .events
                    .front()
                    .is_some_and(|(tick, _)| *tick <= written_tick)
            };
            let after = observations
                .plugins
                .iter()
                .filter(|(id, _)| {
                    observations
                        .last_delivered
                        .as_ref()
                        .is_none_or(|last| *id > last)
                })
                .find(|(_, queue)| ready(queue));
            let id = after
                .or_else(|| observations.plugins.iter().find(|(_, queue)| ready(queue)))
                .map(|(id, _)| id.clone())?;
            let (_, event) = observations.plugins.get_mut(&id)?.events.pop_front()?;
            observations.last_delivered = Some(id);
            drop(observations);
            let input = self.accept_input(ScriptHostInput::Event(event));
            self.observation_turn = false;
            if input.is_some() {
                return input;
            }
        }
    }

    fn try_recv_input(&mut self) -> Result<ScriptHostInput, mpsc::error::TryRecvError> {
        loop {
            if self.observation_turn {
                self.observation_turn = false;
                if let Some(input) = self.next_observation() {
                    return Ok(input);
                }
            }
            if self.coalesced_tick_due {
                self.coalesced_tick_due = false;
                if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                    if let Some(event) =
                        self.accept_monotonic_event(ScriptHostEvent::untargeted(event))
                    {
                        return Ok(ScriptHostInput::Event(event));
                    }
                    continue;
                }
            }
            match self.event_rx.try_recv() {
                Ok(input) => {
                    self.coalesced_tick_due =
                        has_coalesced_server_tick(&self.coalesced_server_tick);
                    if let Some(input) = self.accept_input(input) {
                        return Ok(input);
                    }
                }
                Err(error) => {
                    if let Some(event) = take_coalesced_server_tick(&self.coalesced_server_tick) {
                        if let Some(event) =
                            self.accept_monotonic_event(ScriptHostEvent::untargeted(event))
                        {
                            return Ok(ScriptHostInput::Event(event));
                        }
                        continue;
                    }
                    if let Some(input) = self.next_observation() {
                        return Ok(input);
                    }
                    return Err(error);
                }
            }
        }
    }

    async fn recv_input(&mut self) -> Option<ScriptHostInput> {
        loop {
            match self.try_recv_input() {
                Ok(input) => return Some(input),
                Err(mpsc::error::TryRecvError::Disconnected) => return None,
                Err(mpsc::error::TryRecvError::Empty) => {}
            }
            let Some(input) = self.event_rx.recv().await else {
                continue;
            };
            self.coalesced_tick_due = has_coalesced_server_tick(&self.coalesced_server_tick);
            if let Some(input) = self.accept_input(input) {
                return Some(input);
            }
        }
    }

    fn accept_input(&mut self, input: ScriptHostInput) -> Option<ScriptHostInput> {
        self.observation_turn = true;
        match input {
            ScriptHostInput::Event(event) => event
                .target_is_live(&self.plugin_routes)
                .then(|| self.accept_monotonic_event(event))
                .flatten()
                .map(ScriptHostInput::Event),
            ScriptHostInput::Reload(payload) => Some(ScriptHostInput::Reload(payload)),
            // A pre-commit question is neither an event nor control this consumer
            // interprets: it is carried through unchanged, and the runtime that
            // owns the request answers it.
            ScriptHostInput::Precommit(request) => Some(ScriptHostInput::Precommit(request)),
        }
    }

    fn accept_monotonic_event(&mut self, event: ScriptHostEvent) -> Option<ScriptHostEvent> {
        let ScriptEventKind::ServerTick { tick } = event.kind() else {
            return Some(event);
        };
        if self
            .highest_delivered_tick
            .is_some_and(|highest| *tick <= highest)
        {
            return None;
        }
        self.highest_delivered_tick = Some(*tick);
        Some(event)
    }

    /// Submit a command without blocking the host thread.
    pub fn try_submit_command(
        &self,
        command: ScriptCommand,
    ) -> Result<(), ScriptCommandSubmissionError> {
        if matches!(command, ScriptCommand::HostAttached { .. }) {
            return Err(ScriptCommandSubmissionError::ProvenanceRejected);
        }
        if let Err(error) = command.validate_contract() {
            return Err(ScriptCommandSubmissionError::InvalidCommand { error });
        }
        if let Some(capability) = command.required_capability_kind() {
            return Err(ScriptCommandSubmissionError::PermissionDenied { capability });
        }
        self.command_tx
            .try_send(command)
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => ScriptCommandSubmissionError::QueueFull,
                mpsc::error::TrySendError::Closed(_) => ScriptCommandSubmissionError::QueueClosed,
            })
    }

    /// Attach the identity of the plugin instance whose callback produced this
    /// batch, then cross into server-owned command handling.
    ///
    /// The identity comes from the [`HostCommandAdmission`] the host built for
    /// the instance, never from the guest: a command that already claims
    /// provenance is rejected here, and every command of the batch is checked
    /// against the admission's capabilities before a single one is queued. This
    /// is the one path a runtime-independent plugin host submits through; a
    /// server-side caller with no plugin identity uses
    /// [`Self::try_submit_command`] instead.
    pub fn try_submit_plugin_batch(
        &self,
        admission: &HostCommandAdmission,
        batch: CommandBatch,
    ) -> Result<(), ScriptBatchSubmissionError> {
        for command in batch.commands() {
            if matches!(command, ScriptCommand::HostAttached { .. }) {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::ProvenanceRejected,
                });
            }
            if let Err(error) = command.validate_contract() {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::InvalidCommand { error },
                });
            }
            let denied_capability = command
                .required_capability()
                .filter(|capability| !admission.capabilities().allows(*capability))
                .map(RequiredCommandCapability::kind);
            if let Some(capability) = denied_capability {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::PermissionDenied { capability },
                });
            }
        }

        let command_count = batch.commands().len();
        if command_count == 0 {
            return Ok(());
        }
        let permits = match self.command_tx.try_reserve_many(command_count) {
            Ok(permits) => permits,
            Err(mpsc::error::TrySendError::Full(())) => {
                return Err(ScriptBatchSubmissionError::Full(batch));
            }
            Err(mpsc::error::TrySendError::Closed(())) => {
                return Err(ScriptBatchSubmissionError::Closed(batch));
            }
        };
        let attached = match self.host_admissions.issue(
            admission.plugin_id_arc(),
            batch,
            self.plugin_routes
                .registration(admission.plugin_id())
                .map(|registration| registration.generation),
        ) {
            Ok(attached) => attached,
            Err(batch) => {
                return Err(ScriptBatchSubmissionError::Rejected {
                    batch,
                    error: CommandBatchError::AdmissionUnavailable,
                });
            }
        };
        for (permit, command) in permits.zip(attached) {
            permit.send(command);
        }
        Ok(())
    }

    /// Register the plugin routes (command roots and payload channels) from one
    /// validated plugin manifest.
    pub fn register_plugin_routes(
        &self,
        manifest: &ValidatedScriptPluginManifest,
    ) -> Result<(), ScriptRouteRegistrationError> {
        self.plugin_routes.register(manifest)
    }

    /// Remove every active plugin route owned by one plugin.
    pub fn unregister_plugin_routes(&self, plugin_id: &str) {
        self.plugin_routes.unregister(plugin_id);
    }

    /// Commit a candidate deployment into the live boundary.
    ///
    /// The trusted host builds the replacement off to the side and hands it here:
    /// every command of every staged batch is checked against its own admission
    /// and reserved before the route table changes, the caller's `swap` runs while
    /// the routes already belong to the new generation, and only then are the
    /// attached commands delivered. A refusal therefore leaves the running
    /// generation, its routes and the command queue exactly as they were, and a
    /// committed swap never publishes a route whose generation has not loaded.
    pub fn commit_reload<F>(
        &self,
        manifests: &[ValidatedScriptPluginManifest],
        batches: Vec<(HostCommandAdmission, CommandBatch)>,
        swap: F,
    ) -> Result<(), ScriptReloadCommitError>
    where
        F: FnOnce(),
    {
        for (admission, batch) in &batches {
            for command in batch.commands() {
                if matches!(command, ScriptCommand::HostAttached { .. }) {
                    return Err(ScriptReloadCommitError::Rejected {
                        error: CommandBatchError::ProvenanceRejected,
                    });
                }
                if let Err(error) = command.validate_contract() {
                    return Err(ScriptReloadCommitError::Rejected {
                        error: CommandBatchError::InvalidCommand { error },
                    });
                }
                if let Some(capability) = command
                    .required_capability()
                    .filter(|capability| !admission.capabilities().allows(*capability))
                    .map(RequiredCommandCapability::kind)
                {
                    return Err(ScriptReloadCommitError::Rejected {
                        error: CommandBatchError::PermissionDenied { capability },
                    });
                }
            }
        }

        if self.command_tx.is_closed() {
            return Err(ScriptReloadCommitError::QueueClosed);
        }
        let command_count = batches
            .iter()
            .map(|(_, batch)| batch.commands().len())
            .sum::<usize>();
        if command_count == 0 {
            self.plugin_routes
                .replace_all(manifests)
                .map_err(|error| ScriptReloadCommitError::Ownership { error })?;
            swap();
            return Ok(());
        }

        let permits =
            self.command_tx
                .try_reserve_many(command_count)
                .map_err(|error| match error {
                    mpsc::error::TrySendError::Full(()) => ScriptReloadCommitError::QueueFull,
                    mpsc::error::TrySendError::Closed(()) => ScriptReloadCommitError::QueueClosed,
                })?;
        let mut attached = Vec::with_capacity(command_count);
        for (admission, batch) in batches {
            let issued = self
                .host_admissions
                .issue(admission.plugin_id_arc(), batch, None)
                .map_err(|_| ScriptReloadCommitError::Rejected {
                    error: CommandBatchError::AdmissionUnavailable,
                })?;
            attached.extend(issued);
        }
        // Keep candidate tickets under the same ledger lock while the route
        // generation changes. The server may not accept their queued commands
        // until after another reload; accepting then must not reattribute them.
        let mut pending =
            self.host_admissions
                .pending
                .lock()
                .map_err(|_| ScriptReloadCommitError::Rejected {
                    error: CommandBatchError::AdmissionUnavailable,
                })?;
        self.plugin_routes
            .replace_all(manifests)
            .map_err(|error| ScriptReloadCommitError::Ownership { error })?;
        for command in &attached {
            let ScriptCommand::HostAttached { provenance, .. } = command else {
                unreachable!("candidate batch was host-attached before the route switch");
            };
            pending
                .get_mut(&provenance.nonce())
                .expect("unpublished candidate admission remains reserved")
                .generation = Some(
                self.plugin_routes
                    .registration(provenance.plugin_id())
                    .expect("committed candidate has a live route")
                    .generation,
            );
        }
        drop(pending);
        swap();
        for (permit, command) in permits.zip(attached) {
            permit.send(command);
        }
        Ok(())
    }
}

/// Construct the bounded server/host script boundary.
pub fn script_boundary_pair(
    event_capacity: NonZeroUsize,
    command_capacity: NonZeroUsize,
) -> (ScriptBoundary, ScriptHostEndpoint) {
    let event_capacity = event_capacity.get().min(MAX_SCRIPT_EVENT_QUEUE_CAPACITY);
    let command_capacity = command_capacity
        .get()
        .min(MAX_SCRIPT_COMMAND_QUEUE_CAPACITY);
    let (event_tx, event_rx) = mpsc::channel(event_capacity);
    let (command_tx, command_rx) = mpsc::channel(command_capacity);
    let plugin_routes = PluginRouteAuthority::new();
    let host_admissions = Arc::new(HostAdmissionLedger::default());
    let coalesced_server_tick = Arc::new(StdMutex::new(CoalescedServerTick::default()));
    let observations = Arc::new(StdMutex::new(ObservationQueues::default()));
    let weak_event_tx = event_tx.downgrade();
    (
        ScriptBoundary {
            event_admission: Arc::new(ScriptEventAdmission {
                closed: AtomicBool::new(false),
                sender: StdMutex::new(Some(event_tx)),
                weak_sender: weak_event_tx,
                event_capacity,
                coalesced_server_tick: Arc::clone(&coalesced_server_tick),
            }),
            command_rx: Arc::new(Mutex::new(command_rx)),
            plugin_routes: plugin_routes.clone(),
            host_admissions: Arc::clone(&host_admissions),
            deployed_packages: Arc::from(Vec::new()),
            observations: Arc::clone(&observations),
            precommit: Arc::new(precommit::PrecommitAuthority::new()),
        },
        ScriptHostEndpoint {
            event_rx,
            coalesced_server_tick,
            coalesced_tick_due: false,
            highest_delivered_tick: None,
            command_tx,
            plugin_routes,
            host_admissions,
            observations,
            observation_turn: false,
        },
    )
}

fn take_coalesced_server_tick(slot: &StdMutex<CoalescedServerTick>) -> Option<ScriptEvent> {
    let tick = match slot.lock() {
        Ok(mut slot) => slot.pending.take(),
        Err(poisoned) => poisoned.into_inner().pending.take(),
    }?;
    Some(ScriptEvent::server_tick(tick))
}

fn has_coalesced_server_tick(slot: &StdMutex<CoalescedServerTick>) -> bool {
    match slot.lock() {
        Ok(slot) => slot.pending.is_some(),
        Err(poisoned) => poisoned.into_inner().pending.is_some(),
    }
}

use super::{
    AdmittedScriptCommand, CommandBatch, CommandBatchError, EventTargetGeneration,
    HostAdmissionLedger, HostCommandAdmission, MAX_SCRIPT_COMMAND_QUEUE_CAPACITY,
    MAX_SCRIPT_CUSTOM_PAYLOAD_BYTES, MAX_SCRIPT_EVENT_QUEUE_CAPACITY, PlayerCommandAdmission,
    PluginPackage, RequiredCommandCapability, ScriptBatchSubmissionError, ScriptCommand,
    ScriptCommandAcceptanceError, ScriptCommandSubmissionError, ScriptEvent, ScriptEventKind,
    ScriptHostEvent, ScriptHostInput, ScriptHostInputSender, ScriptPlayerContext, ScriptPlayerId,
    ScriptProtocolPhase, ScriptQueueError, ScriptReloadCommitError,
};
use std::num::NonZeroUsize;
use std::sync::Mutex as StdMutex;
use tokio::sync::{Mutex, mpsc};

impl ScriptHostEvent {
    /// Whether the plugin generation this event was admitted for is still live.
    ///
    /// It reads the route authority directly, which is why it lives here: the
    /// authority and its registration generations are private to this module.
    fn target_is_live(&self, plugin_routes: &PluginRouteAuthority) -> bool {
        match self.target_generation {
            EventTargetGeneration::Untargeted => true,
            EventTargetGeneration::Targeted(Some(generation)) => self
                .event
                .target_plugin_id()
                .is_some_and(|plugin_id| plugin_routes.registration_is_live(plugin_id, generation)),
            EventTargetGeneration::Targeted(None) => false,
        }
    }
}

fn split_player_command(raw: &str) -> Option<(&str, &str)> {
    let command = raw.trim_start();
    let command = command.strip_prefix('/').unwrap_or(command);
    let root_end = command.find(char::is_whitespace).unwrap_or(command.len());
    let root = &command[..root_end];
    if root.is_empty() {
        return None;
    }
    Some((root, command[root_end..].trim_start()))
}

#[cfg(test)]
#[path = "route_authority_tests.rs"]
mod tests;
