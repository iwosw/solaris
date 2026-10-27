//! Connection registry: handles, session locks and lookup.
//! Moved verbatim from `server.rs`.

use super::*;

pub struct ConnectionRegistry {
    pub(super) next_id: AtomicU64,
    pub(super) inner: parking_lot::Mutex<RegistryInner>,
}

#[derive(Default)]
pub(super) struct RegistryInner {
    // A Closing connection retains its slot until native/admin cleanup runs.
    pub(super) admitted: HashSet<ConnectionId>,
    pub(super) joining: HashSet<ConnectionId>,
    pub(super) closers: HashMap<ConnectionId, tokio::sync::oneshot::Sender<()>>,
    pub(super) handles: HashMap<ConnectionId, ConnectionHandles>,
    pub(super) names: HashMap<ConnectionId, String>,
    pub(super) view_distances: HashMap<ConnectionId, i32>,
    /// Client locale last admitted to the core (`ClientInformation.language`);
    /// deduplicates Play updates and is cleared on disconnect.
    pub(super) locales: HashMap<ConnectionId, String>,
    pub(super) roster_dirty: bool,
    // Join state and its waiters share the queue registry's atomic removal.
    pub(super) pending_joins: HashMap<ConnectionId, JoinedSpawn>,
    pub(super) pending_reliable: HashMap<ConnectionId, Vec<mc_core::boundary::PlayerChange>>,
    pub(super) join_waiters: HashMap<ConnectionId, Vec<tokio::sync::oneshot::Sender<JoinedSpawn>>>,
    pub(super) disconnect_reasons: HashMap<ConnectionId, String>,
}

/// Queue handles for one Play connection.
#[derive(Clone)]
pub struct ConnectionHandles {
    /// Reliable queue sender (join tail, acks, menus, health, chat, syncpos, disconnect).
    pub reliable_tx: mpsc::Sender<bytes::Bytes>,
    /// World queue sender (pre-framed chunk/entity packets).
    pub world_tx: mpsc::Sender<bytes::Bytes>,
}

impl ConnectionRegistry {
    /// Empty registry; ids start at 1.
    pub fn new() -> Self {
        Self {
            next_id: AtomicU64::new(FIRST_CONNECTION_ID),
            inner: parking_lot::Mutex::new(RegistryInner::default()),
        }
    }

    /// Allocate the next connection id.
    pub fn alloc_id(&self) -> ConnectionId {
        ConnectionId(self.next_id.fetch_add(1, Ordering::Relaxed))
    }

    /// Reserve a player slot atomically with the actual admission policy.
    /// Joining connections count too; the policy retains operator bypass.
    pub(super) fn admit(
        &self,
        id: ConnectionId,
        maximum: usize,
        check: impl FnOnce(bool) -> Option<serde_json::Value>,
    ) -> Option<serde_json::Value> {
        let mut inner = self.inner.lock();
        let refusal = check(inner.admitted.len() >= maximum);
        if refusal.is_none() {
            inner.admitted.insert(id);
            inner.joining.insert(id);
        }
        refusal
    }

    #[cfg(test)]
    pub(super) fn joining_id(&self) -> ConnectionId {
        let id = self.alloc_id();
        assert!(self.admit(id, usize::MAX, |_| None).is_none());
        id
    }

    /// Register a connection's queue handles and take the reliable changes the
    /// pump stashed before registration, in one critical section with
    /// [`handles_or_stash`][Self::handles_or_stash]: every drained change is
    /// either returned here for replay or routed by the pump through the
    /// registered handles, never stranded in between.
    pub fn register(
        &self,
        id: ConnectionId,
        handles: ConnectionHandles,
    ) -> anyhow::Result<Vec<mc_core::boundary::PlayerChange>> {
        let mut inner = self.inner.lock();
        anyhow::ensure!(
            inner.joining.remove(&id),
            "connection admission already closed"
        );
        inner.handles.insert(id, handles);
        Ok(inner.pending_reliable.remove(&id).unwrap_or_default())
    }

    /// Read actual mpsc occupied-or-reserved slots only on telemetry requests.
    /// Senders cannot reveal queued versus reserved slots, or byte totals.
    pub fn load_queues(&self) -> Option<LoadQueues> {
        let captured_unix_ns = unix_ns();
        let inner = self.inner.lock();
        let mut reliable = LoadQueueGauge::default();
        let mut world = LoadQueueGauge::default();
        for handles in inner.handles.values() {
            for (gauge, sender) in [
                (&mut reliable, &handles.reliable_tx),
                (&mut world, &handles.world_tx),
            ] {
                let capacity = sender.max_capacity();
                let occupied = capacity.saturating_sub(sender.capacity());
                gauge.capacity_slots += capacity;
                gauge.queued_or_reserved_slots += occupied;
                gauge.max_viewer_queued_or_reserved_slots =
                    gauge.max_viewer_queued_or_reserved_slots.max(occupied);
            }
        }
        Some(LoadQueues {
            captured_unix_ns,
            completed_unix_ns: unix_ns(),
            viewers: inner.handles.len(),
            capabilities: LoadQueueCapabilities {
                queued_or_reserved_slots: true,
                queued_slots: false,
                reserved_slots: false,
                queued_bytes: false,
                reserved_bytes: false,
            },
            reliable,
            world,
        })
    }

    /// Stash a Joined the pump drained before the connection registered.
    /// Also fires any registered join waiters for this connection.
    pub(super) fn stash_join(&self, id: ConnectionId, joined: JoinedSpawn) {
        let senders = {
            let mut inner = self.inner.lock();
            if !inner.joining.contains(&id) && !inner.handles.contains_key(&id) {
                return;
            }
            inner.pending_joins.insert(id, joined);
            inner.join_waiters.remove(&id).unwrap_or_default()
        };
        for sender in senders {
            let _ = sender.send(joined);
        }
    }

    /// Take a stashed Joined for a joining connection. Non-removing: the
    /// stashed row doubles as the authoritative entity id for later viewer
    /// construction, so joins stay correct after the first take.
    pub fn take_join(&self, id: ConnectionId) -> Option<JoinedSpawn> {
        self.inner.lock().pending_joins.get(&id).copied()
    }

    /// Pump side of the join race: the handles of a registered connection,
    /// or else stash the drained `changes` for replay at
    /// [`register`][Self::register], atomically with the handle check. Their
    /// `Joined` is published for [`wait_join`][Self::wait_join] only after
    /// that, and the connection registers only after its join wait, so the
    /// stash is always in place before registration takes it.
    pub fn handles_or_stash(
        &self,
        id: ConnectionId,
        changes: &mut Vec<mc_core::boundary::PlayerChange>,
    ) -> Option<ConnectionHandles> {
        let joined = changes.iter().find_map(|change| match change {
            mc_core::boundary::PlayerChange::Joined {
                entity,
                position,
                game_mode,
                dimension,
                ..
            } => Some((*entity, *position, *game_mode, *dimension)),
            _ => None,
        });
        {
            let mut inner = self.inner.lock();
            if let Some(handles) = inner.handles.get(&id) {
                return Some(handles.clone());
            }
            if !inner.joining.contains(&id) {
                changes.clear();
                return None;
            }
            inner
                .pending_reliable
                .entry(id)
                .or_default()
                .append(changes);
        }
        if let Some(joined) = joined {
            self.stash_join(id, joined);
        }
        None
    }

    /// Stash a core-ordered disconnect reason for a connection. The play
    /// loop takes it after flushing the kick packet and logs peer+reason.
    pub fn stash_disconnect_reason(&self, id: ConnectionId, reason: String) {
        let mut inner = self.inner.lock();
        if inner.joining.contains(&id) || inner.handles.contains_key(&id) {
            inner.disconnect_reasons.insert(id, reason);
        }
    }

    /// Take a stashed core-ordered disconnect reason, if the pump routed one.
    pub fn take_disconnect_reason(&self, id: ConnectionId) -> Option<String> {
        self.inner.lock().disconnect_reasons.remove(&id)
    }

    /// Register interest in this connection's Joined. The pump fires the
    /// receiver when it routes the Joined (via [`stash_join`][Self::stash_join]).
    /// Callers must check [`take_join`][Self::take_join] first (fast path
    /// when the Joined already arrived), then await the receiver.
    pub fn wait_join(&self, id: ConnectionId) -> tokio::sync::oneshot::Receiver<JoinedSpawn> {
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let mut inner = self.inner.lock();
        if let Some(joined) = inner.pending_joins.get(&id) {
            let _ = sender.send(*joined);
        } else if inner.joining.contains(&id) {
            inner.join_waiters.entry(id).or_default().push(sender);
        }
        receiver
    }

    /// Stop delivery and wake the connection owner, including a blocked writer.
    /// Admission is released only by the lifecycle owner's final cleanup.
    /// Drops queue senders, the stashed join, stashed pre-registration
    /// reliable changes, any join waiters (their receivers fail closed), and
    /// the stashed disconnect reason.
    pub fn remove(&self, id: ConnectionId) {
        crate::vanilla_items::unmark(id);
        let closer = {
            let mut inner = self.inner.lock();
            inner.joining.remove(&id);
            inner.handles.remove(&id);
            if inner.names.remove(&id).is_some() {
                inner.roster_dirty = true;
            }
            inner.pending_joins.remove(&id);
            inner.pending_reliable.remove(&id);
            inner.join_waiters.remove(&id);
            inner.disconnect_reasons.remove(&id);
            inner.view_distances.remove(&id);
            inner.locales.remove(&id);
            inner.closers.remove(&id)
        };
        if let Some(closer) = closer {
            let _ = closer.send(());
        }
    }

    pub(super) fn finish(&self, id: ConnectionId) {
        self.remove(id);
        self.inner.lock().admitted.remove(&id);
    }

    /// Look up one connection's handles.
    pub fn get(&self, id: ConnectionId) -> Option<ConnectionHandles> {
        self.inner.lock().handles.get(&id).cloned()
    }

    /// Snapshot all handles for pump routing.
    pub fn snapshot(&self) -> Vec<(ConnectionId, ConnectionHandles)> {
        self.inner
            .lock()
            .handles
            .iter()
            .map(|(id, handles)| (*id, handles.clone()))
            .collect()
    }

    // One handle copy per pump iteration; roster work only follows a change.
    pub(super) fn routing_snapshot(
        &self,
    ) -> (
        HashMap<ConnectionId, ConnectionHandles>,
        Option<Vec<String>>,
    ) {
        let (handles, names) = {
            let mut inner = self.inner.lock();
            let names = if std::mem::take(&mut inner.roster_dirty) {
                Some(inner.names.values().cloned().collect::<Vec<_>>())
            } else {
                None
            };
            (inner.handles.clone(), names)
        };
        let names = names.map(|mut names| {
            names.sort();
            names
        });
        (handles, names)
    }

    /// Set a player name for the dashboard roster.
    pub fn set_name(&self, id: ConnectionId, name: String) {
        let mut inner = self.inner.lock();
        if inner.names.get(&id) != Some(&name) {
            inner.names.insert(id, name);
            inner.roster_dirty = true;
        }
    }

    /// Sorted roster names for the dashboard.
    pub fn roster(&self) -> Vec<String> {
        let mut names: Vec<_> = self.inner.lock().names.values().cloned().collect();
        names.sort();
        names
    }

    /// Current connection count.
    pub fn len(&self) -> usize {
        self.inner.lock().handles.len()
    }

    /// Whether no connections are registered.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl Default for ConnectionRegistry {
    fn default() -> Self {
        Self::new()
    }
}
