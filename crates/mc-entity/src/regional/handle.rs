use super::*;

impl RegionalOwnerHandle {
    #[must_use]
    pub fn fatal_error(&self) -> Option<RegionOwnerLaneError> {
        self.worker_health
            .error()
            .and_then(|error| (error == RegionOwnerLaneError::WorkerPanicked).then_some(error))
    }

    pub(super) fn unavailable_error(&self) -> RegionOwnerLaneError {
        self.worker_health.error_after_disconnect()
    }
    fn local_physics_fallback_error(&self, error: RegionOwnerLaneError) -> bool {
        match error {
            RegionOwnerLaneError::StaleLease
            | RegionOwnerLaneError::UnknownRegion
            | RegionOwnerLaneError::Busy => true,
            RegionOwnerLaneError::Closed => self.ensure_operational().is_ok(),
            _ => false,
        }
    }

    fn ensure_operational(&self) -> Result<(), RegionOwnerLaneError> {
        if let Some(error) = self.worker_health.error() {
            return Err(error);
        }
        self.commit_state.ensure_committed_state()
    }

    pub fn snapshot(
        &self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        self.ensure_operational()?;
        if let Some(mut selected) = self.read_cached_selected_entities(&HashSet::from([entity])) {
            return Ok(selected.snapshots.pop());
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Snapshot { entity, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.ensure_operational()?;
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Snapshots { reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn snapshots_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.snapshots_for_ids_selected(entities, SnapshotSelection::All)
    }

    pub fn sheep_grazing_snapshots_for_ids(
        &self,
        entities: &HashSet<EntityId>,
        include_idle: HashSet<EntityId>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.snapshots_for_ids_selected(
            entities,
            SnapshotSelection::SheepGrazing {
                include_idle: Arc::new(include_idle),
            },
        )
    }

    fn snapshots_for_ids_selected(
        &self,
        entities: &HashSet<EntityId>,
        selection: SnapshotSelection,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        if matches!(selection, SnapshotSelection::All)
            && let Some(selected) = self.read_cached_selected_entities(entities)
        {
            return Ok(selected.snapshots);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SnapshotsForIds {
                entities: entities.clone(),
                selection,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result
            .recv()
            .map_err(|_| self.unavailable_error())?
            .map(VersionedEntitySnapshots::into_snapshots)
    }

    pub fn snapshots_for_ids_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<VersionedEntitySnapshots, RegionOwnerLaneError> {
        if let Some(selected) = self.read_cached_selected_entities_versioned(entities) {
            return Ok(selected);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SnapshotsForIds {
                entities: entities.clone(),
                selection: SnapshotSelection::All,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }
    pub fn alive_kinematics_for_ids_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<(Vec<EntityKinematics>, Option<VersionedEntityKinematics>), RegionOwnerLaneError>
    {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::AliveKinematicsForIds {
                entities: entities.clone(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn simulation_results_for_ids_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<
        (
            Vec<EntitySimulationResult>,
            Option<VersionedEntityKinematics>,
        ),
        RegionOwnerLaneError,
    > {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SimulationResultsForIds {
                entities: entities.clone(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    fn read_cached_selected_entities_versioned(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Option<VersionedEntitySnapshots> {
        let selected = self.read_cached_selected_entities(entities)?;
        Some(VersionedEntitySnapshots {
            authority: self.authority,
            fence: selected.fence,
            snapshots: selected.snapshots,
            publication_readers: None,
        })
    }

    /// Cheaply reports whether `selected`'s captured version fence is still
    /// current for this owner: authority must match, every fenced lease must
    /// still route to its captured lane, and every fenced lane's current
    /// state version must equal the captured one. Never fetches or clones
    /// entity snapshots.
    pub fn versioned_snapshots_are_current(
        &self,
        selected: &VersionedEntitySnapshots,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.ensure_operational()?;
        if selected.authority != self.authority {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        if let Some(owner_readers) = selected.publication_readers.as_ref() {
            if !selected.fence.leases.is_empty()
                || !selected.snapshots.is_empty()
                || !owner_readers.keys().eq(selected.fence.lane_versions.keys())
            {
                return Ok(false);
            }
            return Ok(selected.fence.lane_versions.iter().all(|(lane, version)| {
                owner_readers
                    .get(lane)
                    .is_some_and(|owner| owner.state_version() == *version)
            }));
        }
        let routes =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut owners = BTreeMap::<usize, RegionalOwnerLaneReader>::new();
        for (entity, lease) in &selected.fence.leases {
            let Some(route) = routes.get(entity) else {
                return Ok(false);
            };
            if route.lease != *lease {
                return Ok(false);
            }
            owners.insert(lease.lane, route.owner.clone());
        }
        if owners.len() != selected.fence.lane_versions.len() {
            return Ok(false);
        }
        drop(routes);
        Ok(selected.fence.lane_versions.iter().all(|(lane, version)| {
            owners
                .get(lane)
                .is_some_and(|owner| owner.state_version() == *version)
        }))
    }

    pub fn versioned_kinematics_are_current(
        &self,
        selected: &VersionedEntityKinematics,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.ensure_operational()?;
        if selected.authority != self.authority || selected.expected.len() != selected.routes.len()
        {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        let Some(owner_readers) = selected.owner_readers.as_ref() else {
            return Ok(false);
        };
        let mut lanes = BTreeSet::new();
        for (expected, lease) in selected.expected.iter().zip(&selected.routes) {
            if RegionKey::from_position(expected.expected.position) != Some(lease.key)
                || !owner_readers.contains_key(&lease.lane)
            {
                return Ok(false);
            }
            lanes.insert(lease.lane);
        }
        if !lanes.iter().eq(selected.fence.lane_versions.keys()) {
            return Ok(false);
        }
        Ok(selected.fence.lane_versions.iter().all(|(lane, version)| {
            owner_readers
                .get(lane)
                .is_some_and(|owner| owner.state_version() == *version)
        }))
    }

    pub(super) fn read_cached_selected_entities(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Option<SelectedEntitySnapshots> {
        self.commit_state.ensure_committed_state().ok()?;
        let _admission =
            DirectSelectedReadPermit::try_acquire(Arc::clone(&self.direct_selected_reads))?;
        let routes =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut expected = HashMap::with_capacity(ordered.len());
        let mut requests =
            BTreeMap::<usize, (RegionalOwnerLaneReader, Vec<(RegionLease, EntityId)>)>::new();
        for entity in ordered {
            let route = routes.get(&entity)?.clone();
            expected.insert(entity, (route.lease, route.uuid));
            requests
                .entry(route.lease.lane)
                .or_insert_with(|| (route.owner.clone(), Vec::new()))
                .1
                .push((route.lease, entity));
        }
        drop(routes);

        let mut pending = Vec::with_capacity(requests.len());
        let mut lane_versions = BTreeMap::new();
        let mut lane_owners = Vec::with_capacity(requests.len());
        for (lane, (owner, entities)) in requests {
            let version = owner.state_version();
            pending.push(owner.request_snapshots_for_ids(entities).ok()?);
            lane_versions.insert(lane, version);
            lane_owners.push((owner, version));
        }
        #[cfg(test)]
        if let Some(probe) = self
            .selected_read_probe
            .lock()
            .expect("selected-read test probe lock poisoned")
            .take()
        {
            probe.entered.send(()).ok()?;
            probe.release.recv().ok()?;
        }
        let mut snapshots = Vec::with_capacity(expected.len());
        for completion in pending {
            snapshots.extend(completion.recv().ok()?.ok()?);
        }
        if snapshots.len() != expected.len() {
            return None;
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        let mut seen = HashSet::with_capacity(snapshots.len());
        for snapshot in &snapshots {
            let (lease, uuid) = expected.get(&snapshot.id)?;
            if !seen.insert(snapshot.id)
                || snapshot.uuid != *uuid
                || RegionKey::from_position(snapshot.position) != Some(lease.key)
            {
                return None;
            }
        }
        if lane_owners
            .iter()
            .any(|(owner, version)| owner.state_version() != *version)
        {
            #[cfg(test)]
            self.notify_referenced_goal_fallback_probe();
            return None;
        }
        self.commit_state.ensure_committed_state().ok()?;
        let leases = expected
            .iter()
            .map(|(&entity, &(lease, _))| (entity, lease))
            .collect();
        Some(SelectedEntitySnapshots {
            fence: RegionalVersionFence {
                leases,
                lane_versions,
            },
            snapshots,
        })
    }

    #[cfg(test)]
    pub(super) fn pause_selected_read_after_dispatch_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) {
        *self
            .selected_read_probe
            .lock()
            .expect("regional owner test probe lock poisoned") =
            Some(SelectedReadProbe { entered, release });
    }

    #[cfg(test)]
    pub(super) fn notify_referenced_goal_fallback_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
    ) {
        *self
            .referenced_goal_fallback_probe
            .lock()
            .expect("regional owner test probe lock poisoned") = Some(entered);
    }

    #[cfg(test)]
    fn notify_referenced_goal_fallback_probe(&self) {
        if let Some(probe) = self
            .referenced_goal_fallback_probe
            .lock()
            .expect("regional owner test probe lock poisoned")
            .take()
        {
            let _ = probe.send(());
        }
    }

    #[cfg(test)]
    pub(super) fn notify_direct_mutation_admission_for_test(
        &self,
        entered: std::sync::mpsc::Sender<bool>,
    ) {
        *self
            .direct_mutation_admission_probe
            .lock()
            .expect("regional owner test probe lock poisoned") = Some(entered);
    }

    #[cfg(test)]
    fn notify_direct_mutation_admission_probe(&self, owner: &RegionalOwnerLaneReader) {
        let Some(probe) = self
            .direct_mutation_admission_probe
            .lock()
            .expect("regional owner test probe lock poisoned")
            .take()
        else {
            return;
        };
        let blocked = matches!(
            owner.admission().try_lock(),
            Err(std::sync::TryLockError::WouldBlock)
        );
        let _ = probe.send(blocked);
    }

    #[cfg(test)]
    pub(super) fn pause_actor_snapshot_after_read_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) {
        *self
            .actor_snapshot_probe
            .lock()
            .expect("regional owner test probe lock poisoned") =
            Some(SelectedReadProbe { entered, release });
    }

    #[cfg(test)]
    pub(super) fn notify_goal_prepare_entered_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
    ) {
        *self
            .goal_prepare_probe
            .lock()
            .expect("regional owner test probe lock poisoned") = Some(entered);
    }

    #[cfg(test)]
    pub(super) fn notify_goal_apply_topology_for_test(&self, entered: std::sync::mpsc::Sender<()>) {
        *self
            .goal_apply_topology_probe
            .lock()
            .expect("regional owner test probe lock poisoned") = Some(entered);
    }

    #[cfg(test)]
    pub(super) fn hold_coordinator_for_test(
        &self,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.sender
            .send(RegionalOwnerCommand::HoldForTest { entered, release })
            .map_err(|_| self.unavailable_error())
    }

    #[cfg(test)]
    pub(super) fn panic_coordinator_for_test(&self) -> Result<(), RegionOwnerLaneError> {
        self.sender
            .send(RegionalOwnerCommand::PanicForTest)
            .map_err(|_| self.unavailable_error())
    }

    #[cfg(test)]
    pub(super) fn hold_owner_lane_for_test(
        &self,
        entity: EntityId,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.sender
            .send(RegionalOwnerCommand::HoldLaneForTest {
                entity,
                entered,
                release,
            })
            .map_err(|_| self.unavailable_error())
    }

    pub fn contains_uuid(&self, uuid: Uuid) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ContainsUuid { uuid, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    /// Resolve bounded entity UUIDs to their current snapshots. The result keeps
    /// the caller's ordering; an UUID that is not resident resolves to `None`.
    pub fn entities_by_uuid(
        &self,
        uuids: &[Uuid],
    ) -> Result<Vec<Option<EntitySnapshot>>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::EntityByUuid {
                uuids: uuids.to_vec(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn status(&self) -> Result<RegionalOwnerStatus, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Status { reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn reconfigure_lanes(&self, lane_count: usize) -> Result<usize, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ReconfigureLanes { lane_count, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn clear_recovered_commits(
        &self,
        phases: impl IntoIterator<Item = RegionPhase>,
    ) -> Result<(), RegionOwnerLaneError> {
        self.commit_state
            .clear_recovered_commits(&phases.into_iter().collect::<Vec<_>>())
    }

    pub fn save_barrier(&self) -> Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SaveBarrier { reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn advance_lifecycle_epoch(
        &self,
        lifecycle_epoch: u64,
    ) -> Result<(), RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::AdvanceLifecycleEpoch {
                lifecycle_epoch,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn restore_checkpoint_boundary(
        &self,
        lifecycle_epoch: u64,
        sequence_watermark: u64,
    ) -> Result<(), RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::RestoreCheckpointBoundary {
                lifecycle_epoch,
                sequence_watermark,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn spawn(&self, entity: SpawnEntity) -> Result<EntityId, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Spawn {
                entity: Box::new(entity),
                defer_journal: false,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn spawn_deferred_journal(
        &self,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Spawn {
                entity: Box::new(entity),
                defer_journal: true,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn spawn_batch(
        &self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnBatch {
                entities: entities.into_iter().collect(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn spawn_unique_batch(
        &self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SpawnUniqueBatch {
                entities: entities.into_iter().collect(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn insert_snapshots_batch(
        &self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::InsertSnapshots {
                snapshots: snapshots.into_iter().collect(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn remove(&self, entity: EntityId) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Remove { entity, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn remove_if_current(
        &self,
        expected: EntitySnapshot,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::RemoveIfCurrent {
                expected: Box::new(expected),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn prepare_snapshot_if_current(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<Option<PreparedSnapshotMutation>, RegionOwnerLaneError> {
        let entity = expected.id;
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::PrepareSnapshot {
                expected: Box::new(expected),
                next: Box::new(next),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        Ok(result
            .recv()
            .map_err(|_| self.unavailable_error())??
            .map(|(phase, lane, sequence)| PreparedSnapshotMutation {
                owner: self.clone(),
                phase,
                lane,
                sequence,
                entity,
                committed: false,
            }))
    }

    pub fn replace_snapshot_if_current(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.replace_snapshot_if_current_inner(expected, next, true, false)
    }

    pub fn replace_snapshot_if_current_deferred_journal(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.replace_snapshot_if_current_inner(expected, next, false, false)
    }

    pub fn convert_snapshot_if_current(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.replace_snapshot_if_current_inner(expected, next, true, true)
    }

    fn replace_snapshot_if_current_inner(
        &self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
        journal_commit: bool,
        allow_type_change: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ReplaceSnapshotIfCurrent {
                expected: Box::new(expected),
                next: Box::new(next),
                defer_journal: !journal_commit,
                allow_type_change,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn replace_snapshots_if_current(
        &self,
        snapshots: impl IntoIterator<Item = (EntitySnapshot, EntitySnapshot)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        if snapshots.is_empty() {
            return Ok(true);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ReplaceSnapshotsIfCurrent { snapshots, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn merge_item_snapshots_if_current(
        &self,
        survivor_expected: EntitySnapshot,
        survivor_next: EntitySnapshot,
        consumed_expected: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::MergeItemSnapshotsIfCurrent {
                survivor_expected: Box::new(survivor_expected),
                survivor_next: Box::new(survivor_next),
                consumed_expected: Box::new(consumed_expected),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_villager_birth_if_current(
        &self,
        commit: VillagerBirthCommit,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CommitVillagerBirthIfCurrent {
                commit: Box::new(commit),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_villager_inventory_pickup_if_current(
        &self,
        commit: VillagerInventoryPickupCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(
                RegionalOwnerCommand::CommitVillagerInventoryPickupIfCurrent {
                    commit: Box::new(commit),
                    reply,
                },
            )
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_villager_food_share_if_current(
        &self,
        commit: VillagerFoodShareCommit,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CommitVillagerFoodShareIfCurrent {
                commit: Box::new(commit),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_villager_courtship_if_current(
        &self,
        commit: VillagerCourtshipCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CommitVillagerCourtshipIfCurrent {
                commit: Box::new(commit),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_villager_no_bed_if_current(
        &self,
        commit: VillagerNoBedCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CommitVillagerNoBedIfCurrent {
                commit: Box::new(commit),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_animal_states_if_current(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, true)
    }

    pub fn set_animal_states_if_current_deferred_journal(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, false)
    }

    fn set_animal_states_if_current_inner(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        let expected_count = states.len();
        let direct = states
            .iter()
            .map(|(expected, animal)| {
                (
                    expected.clone(),
                    RegionOwnerMutation::SetAnimalStateIfCurrent {
                        expected: Box::new(expected.clone()),
                        animal: *animal,
                    },
                )
            })
            .collect();
        if let Some(result) =
            self.try_commit_cached_snapshot_mutations_with_journal(direct, journal_commit)
        {
            return result.map(|snapshots| snapshots.len() == expected_count);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetAnimalStatesIfCurrent {
                states,
                defer_journal: !journal_commit,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_goal(
        &self,
        entity: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoal {
                entity,
                goal,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_goals(
        &self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let ids = goals
            .iter()
            .map(|(entity, _)| *entity)
            .collect::<HashSet<_>>();
        let mut selected = ids.clone();
        selected.extend(goals.iter().filter_map(|(_, goal)| goal_reference(goal)));
        if ids.len() == goals.len()
            && let Some(selected) = self.read_cached_selected_entities_versioned(&selected)
        {
            let current = selected
                .snapshots()
                .iter()
                .cloned()
                .map(|snapshot| (snapshot.id, snapshot))
                .collect::<HashMap<_, _>>();
            let direct = goals
                .iter()
                .map(|(entity, goal)| {
                    let expected = current[entity].clone();
                    (
                        expected.clone(),
                        RegionOwnerMutation::SetGoalIfCurrent {
                            expected: Box::new(expected),
                            goal: goal.clone(),
                        },
                    )
                })
                .collect();
            let referenced = goals.iter().any(|(_, goal)| goal_reference(goal).is_some());
            let result = if referenced {
                self.try_commit_cached_referenced_snapshot_mutations(direct, &selected)
            } else {
                self.try_commit_cached_snapshot_mutations(direct)
            };
            if let Some(result) = result {
                return result.map(|snapshots| snapshots.len());
            }
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal: false,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_goals_deferred_journal(
        &self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal: true,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_item_stack(
        &self,
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetItemStack {
                entity,
                item_stack,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_item_stack_if_current(
        &self,
        expected: EntitySnapshot,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        if expected.retained.item_pickup_claim.is_some() {
            return Ok(false);
        }
        if let Some(result) =
            self.try_commit_cached_snapshot_mutation(expected.clone(), |expected| {
                RegionOwnerMutation::SetItemStackIfCurrent {
                    expected: Box::new(expected),
                    item_stack: item_stack.clone(),
                }
            })
        {
            return result.map(|snapshots| snapshots.len() == 1);
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetItemStackIfCurrent {
                expected: Box::new(expected),
                item_stack,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn resolve_item_pickup_claim(
        &self,
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
    ) -> Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError> {
        self.resolve_item_pickup_claim_inner(entity, claim, item_stack, true)
    }

    pub fn resolve_item_pickup_claim_deferred_journal(
        &self,
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
    ) -> Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError> {
        self.resolve_item_pickup_claim_inner(entity, claim, item_stack, false)
    }

    fn resolve_item_pickup_claim_inner(
        &self,
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
        journal_commit: bool,
    ) -> Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ResolveItemPickupClaim {
                entity,
                claim,
                item_stack,
                defer_journal: !journal_commit,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }
    fn try_commit_cached_snapshot_mutation(
        &self,
        expected: EntitySnapshot,
        mutation: impl FnOnce(EntitySnapshot) -> RegionOwnerMutation,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        let direct_mutation = mutation(expected.clone());
        self.try_commit_cached_snapshot_mutations(vec![(expected, direct_mutation)])
    }

    fn try_commit_cached_snapshot_mutations(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_with_journal(expected_mutations, true)
    }

    fn try_commit_cached_snapshot_mutations_with_journal(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_inner(
            expected_mutations,
            journal_commit,
            false,
            false,
            false,
        )
        .map(|result| result.map(CachedSnapshotCommit::into_snapshots))
    }

    fn try_commit_cached_standalone_snapshot_mutations_with_journal_versioned(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
    ) -> Option<Result<CachedSnapshotCommit, RegionOwnerLaneError>> {
        self.try_commit_cached_snapshot_mutations_inner(
            expected_mutations,
            journal_commit,
            true,
            false,
            false,
        )
    }

    fn try_commit_cached_referenced_snapshot_mutations(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        selected: &VersionedEntitySnapshots,
    ) -> Option<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>> {
        if selected.authority != self.authority
            || selected.fence.leases.len() != selected.snapshots.len()
        {
            return None;
        }
        let _mutation_gate =
            read_authoritative_rwlock(&self.mutation_gate, "regional.mutation_gate");
        let routes =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut owners = BTreeMap::new();
        for snapshot in selected.snapshots() {
            let route = routes.get(&snapshot.id)?;
            if route.uuid != snapshot.uuid
                || RegionKey::from_position(snapshot.position) != Some(route.lease.key)
                || selected.fence.leases.get(&snapshot.id) != Some(&route.lease)
                || !selected.fence.lane_versions.contains_key(&route.lease.lane)
            {
                return None;
            }
            owners
                .entry(route.lease.lane)
                .or_insert_with(|| route.owner.clone());
        }
        if owners.len() != selected.fence.lane_versions.len() {
            return None;
        }
        drop(routes);
        let _lane_admissions = owners
            .values()
            .map(|owner| {
                lock_authoritative_mutex(owner.admission(), "regional.owner_lane_admission")
            })
            .collect::<Vec<_>>();
        if owners.iter().any(|(lane, owner)| {
            selected
                .fence
                .lane_versions
                .get(lane)
                .is_none_or(|version| owner.state_version() != *version)
        }) {
            #[cfg(test)]
            self.notify_referenced_goal_fallback_probe();
            return None;
        }
        self.try_commit_cached_snapshot_mutations_inner(expected_mutations, true, false, true, true)
            .map(|result| result.map(CachedSnapshotCommit::into_snapshots))
    }

    fn try_commit_cached_snapshot_mutations_inner(
        &self,
        expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
        journal_commit: bool,
        require_standalone: bool,
        mutation_gate_held: bool,
        lane_admission_held: bool,
    ) -> Option<Result<CachedSnapshotCommit, RegionOwnerLaneError>> {
        if expected_mutations.is_empty() {
            return Some(Ok(CachedSnapshotCommit {
                fence: RegionalVersionFence {
                    leases: HashMap::new(),
                    lane_versions: BTreeMap::new(),
                },
                snapshots: Vec::new(),
            }));
        }
        let _mutation_gate = (!mutation_gate_held)
            .then(|| read_authoritative_rwlock(&self.mutation_gate, "regional.mutation_gate"));
        let cached =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut routes = BTreeMap::new();
        let mut mutation_leases = Vec::with_capacity(expected_mutations.len());
        let mut selected_lane = None;
        for (fallback, mutation) in &expected_mutations {
            if matches!(
                mutation,
                RegionOwnerMutation::SetKinematicsBatchIfVersion { .. }
            ) {
                return None;
            }
            let expected = cached_mutation_expected_snapshots(fallback, mutation);
            if expected.first() != Some(fallback) {
                return Some(Err(RegionOwnerLaneError::InvalidMutation));
            }
            let mut mutation_lease = None;
            for snapshot in expected {
                let Some(key) = RegionKey::from_position(snapshot.position) else {
                    return Some(Err(RegionOwnerLaneError::InvalidMutation));
                };
                let route = cached.get(&snapshot.id)?.clone();
                if route.lease.key != key || route.uuid != snapshot.uuid {
                    return None;
                }
                if require_standalone && !route.standalone {
                    return None;
                }
                if selected_lane.is_some_and(|lane| lane != route.lease.lane) {
                    return None;
                }
                if mutation_lease.is_some_and(|lease| lease != route.lease) {
                    return None;
                }
                if routes.insert(snapshot.id, route.clone()).is_some() {
                    return Some(Err(RegionOwnerLaneError::InvalidMutation));
                }
                selected_lane = Some(route.lease.lane);
                mutation_lease = Some(route.lease);
            }
            let Some(mutation_lease) = mutation_lease else {
                return Some(Err(RegionOwnerLaneError::InvalidMutation));
            };
            mutation_leases.push(mutation_lease);
        }
        drop(cached);
        let owner = routes.values().next()?.owner.clone();
        Some((|| {
            #[cfg(test)]
            self.notify_direct_mutation_admission_probe(&owner);
            let _lane_admission = (!lane_admission_held).then(|| {
                lock_authoritative_mutex(owner.admission(), "regional.owner_lane_admission")
            });
            let (first_sequence, sequence) = self
                .commit_state
                .reserve_sequences(expected_mutations.len())?;
            let phase = self.commit_state.reserve_phase()?;
            let expected_post = routes
                .iter()
                .map(|(&entity, route)| (entity, (route.lease, route.uuid)))
                .collect::<BTreeMap<_, _>>();
            let requested = expected_post
                .iter()
                .map(|(&entity, &(lease, _))| (lease, entity))
                .collect::<Vec<_>>();
            let mutations = expected_mutations
                .into_iter()
                .zip(mutation_leases)
                .enumerate()
                .map(|(offset, ((_, mutation), lease))| SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation,
                })
                .collect();
            let committed = owner.prepare_and_commit(RegionOwnerBatch {
                phase,
                sequence_watermark: sequence,
                mutations,
            })?;
            match committed.recv().map_err(|_| self.unavailable_error())? {
                Ok(completion) if completion.phase == phase => {}
                Ok(_) => return Err(RegionOwnerLaneError::StalePhase),
                Err(RegionOwnerLaneError::InvalidMutation) => {
                    return Ok(CachedSnapshotCommit {
                        fence: RegionalVersionFence {
                            leases: HashMap::new(),
                            lane_versions: BTreeMap::new(),
                        },
                        snapshots: Vec::new(),
                    });
                }
                Err(error) => return Err(error),
            }

            let post_state = owner
                .request_existing_snapshots_for_ids(requested)
                .and_then(|snapshots| snapshots.recv().map_err(|_| self.unavailable_error())?);
            let upserts = match post_state {
                Ok(upserts)
                    if upserts.len() == expected_post.len()
                        && upserts.iter().all(|snapshot| {
                            expected_post
                                .get(&snapshot.id)
                                .is_some_and(|(lease, uuid)| {
                                    snapshot.uuid == *uuid
                                        && RegionKey::from_position(snapshot.position)
                                            == Some(lease.key)
                                })
                        }) =>
                {
                    upserts
                }
                Ok(_) => {
                    owner.rollback_committed(phase)?;
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                Err(error) => {
                    owner.rollback_committed(phase)?;
                    return Err(error);
                }
            };
            let journal_enabled = journal_commit
                && lock_authoritative_mutex(
                    &self.commit_state.journal,
                    "regional.decision_journal",
                )
                .enabled();
            if journal_enabled {
                let decision = RegionalCommitDecision {
                    phase,
                    sequence_watermark: sequence,
                    lifecycle_epoch: self.commit_state.lifecycle_epoch(),
                    upserts: upserts.clone(),
                    removed: Vec::new(),
                };
                let journal_result = self.commit_state.record_commit(&decision);
                if let Err(error) = journal_result {
                    if error.outcome_unknown() {
                        lock_authoritative_mutex(
                            &self.commit_state.journal_commits,
                            "regional.journal_commit_identities",
                        )
                        .insert(phase, decision.identity());
                        self.commit_state
                            .outcome_unknown
                            .store(true, Ordering::Release);
                        return Err(RegionOwnerLaneError::Journal);
                    }
                    owner.rollback_committed(phase)?;
                    return Err(RegionOwnerLaneError::Journal);
                }
            }
            let finalized = owner.finalize(phase)?;
            match finalized.recv().map_err(|_| self.unavailable_error())? {
                Ok(finalized) if finalized == phase => {
                    let lane = selected_lane.ok_or(RegionOwnerLaneError::WrongLane)?;
                    let leases = upserts
                        .iter()
                        .map(|snapshot| (snapshot.id, routes[&snapshot.id].lease))
                        .collect();
                    let lane_versions = BTreeMap::from([(lane, owner.state_version())]);
                    Ok(CachedSnapshotCommit {
                        fence: RegionalVersionFence {
                            leases,
                            lane_versions,
                        },
                        snapshots: upserts,
                    })
                }
                Ok(_) => Err(RegionOwnerLaneError::StalePhase),
                Err(error) => Err(error),
            }
        })())
    }

    fn rollback_multi_lane_commits(
        &self,
        committed_lanes: &[(RegionalOwnerLaneReader, RegionPhase)],
    ) -> Result<(), RegionOwnerLaneError> {
        let mut rollback_failed = false;
        for (owner, phase) in committed_lanes {
            rollback_failed |= owner.rollback_committed(*phase).is_err();
        }
        if rollback_failed {
            self.commit_state.mark_outcome_unknown();
            Err(RegionOwnerLaneError::OutcomeUnknown)
        } else {
            Ok(())
        }
    }

    /// Two-phase commit of cached standalone owner-lane mutations across
    /// multiple lanes. Lane admissions are acquired in ascending lane order
    /// and held across prepare, validate, and finalize. Every lane prepares
    /// first; only after every lane prepared and validated its post state do
    /// the phases finalize. Normal stale or failed preparation rolls every
    /// prepared phase back before admissions release. A rollback or finalize
    /// failure makes the transaction outcome indeterminate and fail-stops the
    /// owner runtime instead of permitting fallback or retry. Deferred-journal:
    /// no decision journal write is recorded. Returns `None` when cached routes
    /// no longer match, so the caller can fall back to the serial path.
    fn try_commit_multi_lane_cached_standalone_snapshot_mutations_deferred_journal(
        &self,
        lane_mutations: Vec<(usize, Vec<(EntitySnapshot, RegionOwnerMutation)>)>,
        max_workers: usize,
    ) -> Option<Result<VersionedEntitySnapshots, RegionOwnerLaneError>> {
        struct LaneResolution {
            owner: RegionalOwnerLaneReader,
            entities: Vec<EntityId>,
            expected_mutations: Vec<(EntitySnapshot, RegionOwnerMutation)>,
            mutation_leases: Vec<RegionLease>,
        }
        struct LaneCommit {
            owner: RegionalOwnerLaneReader,
            phase: RegionPhase,
            batch: RegionOwnerBatch,
            requested: Vec<(RegionLease, EntityId)>,
            expected_post: BTreeMap<EntityId, (RegionLease, Uuid)>,
        }
        enum LanePrepareOutcome {
            Prepared(RegionalOwnerLaneReader, RegionPhase),
            Rejected(RegionOwnerLaneError),
            Indeterminate(RegionalOwnerLaneReader, RegionPhase),
        }
        let _mutation_gate =
            read_authoritative_rwlock(&self.mutation_gate, "regional.mutation_gate");
        let cached =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut routes = BTreeMap::<EntityId, CachedEntityReadRoute>::new();
        let mut lanes = Vec::<LaneResolution>::with_capacity(lane_mutations.len());
        for (_, expected_mutations) in lane_mutations {
            let mut mutation_leases = Vec::with_capacity(expected_mutations.len());
            let mut entities = Vec::with_capacity(expected_mutations.len());
            let mut lane = None;
            let mut lane_owner = None;
            for (fallback, mutation) in &expected_mutations {
                if matches!(
                    mutation,
                    RegionOwnerMutation::SetKinematicsBatchIfVersion { .. }
                ) {
                    return None;
                }
                let expected = cached_mutation_expected_snapshots(fallback, mutation);
                if expected.first() != Some(fallback) {
                    return Some(Err(RegionOwnerLaneError::InvalidMutation));
                }
                let mut mutation_lease = None;
                for snapshot in expected {
                    let Some(key) = RegionKey::from_position(snapshot.position) else {
                        return Some(Err(RegionOwnerLaneError::InvalidMutation));
                    };
                    let route = cached.get(&snapshot.id)?;
                    if route.lease.key != key
                        || route.uuid != snapshot.uuid
                        || !route.standalone
                        || lane.is_some_and(|group_lane| group_lane != route.lease.lane)
                    {
                        return None;
                    }
                    if mutation_lease.is_some_and(|lease| lease != route.lease) {
                        return Some(Err(RegionOwnerLaneError::InvalidMutation));
                    }
                    if routes.insert(snapshot.id, route.clone()).is_some() {
                        return Some(Err(RegionOwnerLaneError::InvalidMutation));
                    }
                    lane = Some(route.lease.lane);
                    mutation_lease = Some(route.lease);
                    entities.push(snapshot.id);
                    lane_owner.get_or_insert_with(|| route.owner.clone());
                }
                let Some(mutation_lease) = mutation_lease else {
                    return Some(Err(RegionOwnerLaneError::InvalidMutation));
                };
                mutation_leases.push(mutation_lease);
            }
            let Some(owner) = lane_owner else {
                return Some(Err(RegionOwnerLaneError::InvalidMutation));
            };
            lanes.push(LaneResolution {
                owner,
                entities,
                expected_mutations,
                mutation_leases,
            });
        }
        drop(cached);

        // Lane admissions are acquired in ascending lane order and held
        // across prepare, validate, and finalize so no other direct mutation
        // interleaves on any lane of this batch.
        let _lane_admissions = lanes
            .iter()
            .map(|lane| {
                #[cfg(test)]
                self.notify_direct_mutation_admission_probe(&lane.owner);
                lock_authoritative_mutex(lane.owner.admission(), "regional.owner_lane_admission")
            })
            .collect::<Vec<_>>();

        let mut commits = Vec::<LaneCommit>::with_capacity(lanes.len());
        for lane in &lanes {
            let (first_sequence, sequence) = match self
                .commit_state
                .reserve_sequences(lane.expected_mutations.len())
            {
                Ok(reserved) => reserved,
                Err(error) => return Some(Err(error)),
            };
            let phase = match self.commit_state.reserve_phase() {
                Ok(phase) => phase,
                Err(error) => return Some(Err(error)),
            };
            let expected_post = lane
                .entities
                .iter()
                .map(|entity| {
                    let route = &routes[entity];
                    (*entity, (route.lease, route.uuid))
                })
                .collect::<BTreeMap<_, _>>();
            let requested = expected_post
                .iter()
                .map(|(&entity, &(lease, _))| (lease, entity))
                .collect::<Vec<_>>();
            let batch = RegionOwnerBatch {
                phase,
                sequence_watermark: sequence,
                mutations: lane
                    .expected_mutations
                    .iter()
                    .zip(&lane.mutation_leases)
                    .enumerate()
                    .map(|(offset, ((_, mutation), lease))| SequencedRegionMutation {
                        sequence: first_sequence + offset as u64 + 1,
                        lease: *lease,
                        mutation: mutation.clone(),
                    })
                    .collect(),
            };
            commits.push(LaneCommit {
                owner: lane.owner.clone(),
                phase,
                batch,
                requested,
                expected_post,
            });
        }

        let mut committed_lanes =
            Vec::<(RegionalOwnerLaneReader, RegionPhase)>::with_capacity(commits.len());
        let mut prepare_lanes = Vec::with_capacity(commits.len());
        let mut validate_lanes = Vec::with_capacity(commits.len());
        for commit in commits {
            committed_lanes.push((commit.owner.clone(), commit.phase));
            prepare_lanes.push((commit.owner.clone(), commit.batch, commit.phase));
            validate_lanes.push((
                commit.owner,
                commit.phase,
                commit.requested,
                commit.expected_post,
            ));
        }

        // Phase 1: prepare every lane. Nothing is finalized yet, so any
        // stale, failed, or errored lane rolls the whole batch back to its
        // pre-call state.
        let prepare_outcomes = self.run_deferred_lane_wave(
            prepare_lanes,
            max_workers,
            |_handle, (owner, batch, phase)| match owner.prepare_and_commit(batch) {
                Ok(committed) => match committed.recv() {
                    Ok(Ok(completion)) if completion.phase == phase => {
                        LanePrepareOutcome::Prepared(owner, phase)
                    }
                    Ok(Ok(_)) | Err(_) => LanePrepareOutcome::Indeterminate(owner, phase),
                    Ok(Err(error)) => LanePrepareOutcome::Rejected(error),
                },
                Err(error) => LanePrepareOutcome::Rejected(error),
            },
        );
        let mut prepared_lanes = Vec::with_capacity(committed_lanes.len());
        let mut hard_error = None;
        let mut stale_expected = false;
        let mut wave_outcome_unknown = false;
        for chunk in prepare_outcomes {
            match chunk {
                Ok(results) => {
                    for result in results {
                        match result {
                            LanePrepareOutcome::Prepared(owner, phase) => {
                                prepared_lanes.push((owner, phase));
                            }
                            LanePrepareOutcome::Rejected(RegionOwnerLaneError::InvalidMutation) => {
                                stale_expected = true
                            }
                            LanePrepareOutcome::Rejected(error) => {
                                hard_error.get_or_insert(error);
                            }
                            LanePrepareOutcome::Indeterminate(owner, phase) => {
                                prepared_lanes.push((owner, phase));
                                wave_outcome_unknown = true;
                            }
                        }
                    }
                }
                Err(_) => wave_outcome_unknown = true,
            }
        }
        if stale_expected || hard_error.is_some() || wave_outcome_unknown {
            if let Err(error) = self.rollback_multi_lane_commits(&prepared_lanes) {
                return Some(Err(error));
            }
            if wave_outcome_unknown {
                self.commit_state.mark_outcome_unknown();
                return Some(Err(RegionOwnerLaneError::OutcomeUnknown));
            }
            return Some(match hard_error {
                Some(error) => Err(error),
                None => Ok(VersionedEntitySnapshots {
                    authority: self.authority,
                    fence: RegionalVersionFence {
                        leases: HashMap::new(),
                        lane_versions: BTreeMap::new(),
                    },
                    snapshots: Vec::new(),
                    publication_readers: None,
                }),
            });
        }

        // Phase 2: validate every lane's post state while nothing is
        // finalized, so a mismatch still rolls the whole batch back.
        let validate_outcomes = self.run_deferred_lane_wave(
            validate_lanes,
            max_workers,
            |handle, (owner, _, requested, expected_post)| {
                let post_state = owner.request_existing_snapshots_for_ids(requested);
                match post_state {
                    Ok(snapshots) => match snapshots.recv() {
                        Ok(Ok(upserts))
                            if upserts.len() == expected_post.len()
                                && upserts.iter().all(|snapshot| {
                                    expected_post
                                        .get(&snapshot.id)
                                        .is_some_and(|(lease, uuid)| {
                                            snapshot.uuid == *uuid
                                                && RegionKey::from_position(snapshot.position)
                                                    == Some(lease.key)
                                        })
                                }) =>
                        {
                            Ok(upserts)
                        }
                        Ok(Ok(_)) => Err(RegionOwnerLaneError::InvalidMutation),
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err(handle.unavailable_error()),
                    },
                    Err(error) => Err(error),
                }
            },
        );
        let mut upserts = Vec::new();
        let mut validate_error = None;
        for chunk in validate_outcomes {
            match chunk {
                Ok(results) => {
                    for result in results {
                        match result {
                            Ok(lane_upserts) => upserts.extend(lane_upserts),
                            Err(error) => {
                                validate_error.get_or_insert(error);
                            }
                        }
                    }
                }
                Err(error) => {
                    validate_error.get_or_insert(error);
                }
            }
        }
        if let Some(error) = validate_error {
            if let Err(rollback_error) = self.rollback_multi_lane_commits(&prepared_lanes) {
                return Some(Err(rollback_error));
            }
            return Some(Err(error));
        }

        // Phase 3: every lane prepared and validated, so finalize all phases.
        let finalize_outcomes =
            self.run_deferred_lane_wave(prepared_lanes, max_workers, |handle, (owner, phase)| {
                match owner.finalize(phase) {
                    Ok(finalized) => match finalized.recv() {
                        Ok(Ok(finalized)) if finalized == phase => Ok(()),
                        Ok(Ok(_)) => Err(RegionOwnerLaneError::StalePhase),
                        Ok(Err(error)) => Err(error),
                        Err(_) => Err(handle.unavailable_error()),
                    },
                    Err(error) => Err(error),
                }
            });
        let mut finalize_error = None;
        for chunk in finalize_outcomes {
            match chunk {
                Ok(results) => {
                    for result in results {
                        if let Err(error) = result {
                            finalize_error.get_or_insert(error);
                        }
                    }
                }
                Err(error) => {
                    finalize_error.get_or_insert(error);
                }
            }
        }
        if finalize_error.is_some() {
            // Some lanes may already be irreversible. Fence every later read,
            // mutation, fallback, and retry until controlled recovery.
            self.commit_state.mark_outcome_unknown();
            return Some(Err(RegionOwnerLaneError::OutcomeUnknown));
        }
        upserts.sort_unstable_by_key(|snapshot| snapshot.id);
        let mut leases = HashMap::new();
        let mut lane_versions = BTreeMap::new();
        for snapshot in &upserts {
            let route = &routes[&snapshot.id];
            leases.insert(snapshot.id, route.lease);
            lane_versions
                .entry(route.lease.lane)
                .or_insert_with(|| route.owner.state_version());
        }
        Some(Ok(VersionedEntitySnapshots {
            authority: self.authority,
            fence: RegionalVersionFence {
                leases,
                lane_versions,
            },
            snapshots: upserts,
            publication_readers: None,
        }))
    }

    /// Runs one phase of the multi-lane commit as at most `max_workers`
    /// scoped workers over lane chunks. Returns per-chunk results in input
    /// order; a panicked worker reports `Err` for its chunk while the other
    /// chunks still complete.
    fn run_deferred_lane_wave<T, R>(
        &self,
        items: Vec<T>,
        max_workers: usize,
        work: impl Fn(&RegionalOwnerHandle, T) -> R + Send + Sync,
    ) -> Vec<Result<Vec<R>, RegionOwnerLaneError>>
    where
        T: Send,
        R: Send,
    {
        if items.is_empty() {
            return Vec::new();
        }
        let worker_count = max_workers.min(items.len()).max(1);
        let chunk_size = items.len().div_ceil(worker_count);
        let mut chunks = Vec::with_capacity(worker_count);
        let mut remaining = items;
        while !remaining.is_empty() {
            let split_at = chunk_size.min(remaining.len());
            chunks.push(remaining.drain(..split_at).collect::<Vec<_>>());
        }
        std::thread::scope(|scope| {
            let work = &work;
            let mut workers = Vec::with_capacity(chunks.len());
            for chunk in chunks {
                let handle = self.clone();
                workers.push(scope.spawn(move || {
                    chunk
                        .into_iter()
                        .map(|item| work(&handle, item))
                        .collect::<Vec<R>>()
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().map_err(|_| self.unavailable_error()))
                .collect()
        })
    }

    pub fn set_position(
        &self,
        entity: EntityId,
        position: Vec3,
    ) -> Result<bool, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetPosition {
                entity,
                position,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn set_velocities(
        &self,
        velocities: impl IntoIterator<Item = (EntityId, Vec3)>,
    ) -> Result<(), RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::SetVelocities {
                velocities: velocities.into_iter().collect(),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn apply_kinematics_if_current(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        let expected_count = states.len();
        self.apply_kinematics_if_current_inner(states, true)
            .map(|committed| committed.len() == expected_count)
    }

    fn empty_versioned_snapshots(&self) -> VersionedEntitySnapshots {
        VersionedEntitySnapshots {
            authority: self.authority,
            fence: RegionalVersionFence {
                leases: HashMap::new(),
                lane_versions: BTreeMap::new(),
            },
            snapshots: Vec::new(),
            publication_readers: None,
        }
    }

    fn apply_kinematics_if_current_inner(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        journal_commit: bool,
    ) -> Result<Vec<EntityKinematics>, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner_versioned(states, journal_commit)
            .map(|selected| {
                selected
                    .into_snapshots()
                    .into_iter()
                    .map(snapshot_kinematics)
                    .collect()
            })
    }

    fn apply_kinematics_if_current_inner_versioned(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        journal_commit: bool,
    ) -> Result<VersionedEntitySnapshots, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(self.empty_versioned_snapshots());
        }
        let expected_count = states.len();
        let mut ids = HashSet::with_capacity(expected_count);
        let mut direct_eligible = true;
        for (expected, state) in &states {
            let source = RegionKey::from_position(expected.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if expected.id != state.id || !state.is_finite() || !ids.insert(expected.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if expected.retained.item_pickup_claim.is_some() && state.position != expected.position
            {
                return Ok(self.empty_versioned_snapshots());
            }
            direct_eligible &= RegionKey::from_position(state.position) == Some(source);
        }
        if direct_eligible {
            let direct = kinematics_direct_mutations(&states)?;
            if let Some(result) = self
                .try_commit_cached_standalone_snapshot_mutations_with_journal_versioned(
                    direct,
                    journal_commit,
                )
            {
                return result.map(|commit| {
                    if commit.snapshots.len() != expected_count {
                        self.empty_versioned_snapshots()
                    } else {
                        commit.into_versioned(self.authority)
                    }
                });
            }
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyKinematicsIfCurrent {
                states,
                defer_journal: !journal_commit,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn apply_kinematics_if_current_deferred_journal_committed(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<Vec<EntityKinematics>, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner(states, false)
    }

    pub fn apply_kinematics_if_current_deferred_journal_committed_versioned(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<VersionedEntitySnapshots, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner_versioned(states, false)
    }

    pub fn try_apply_kinematics_if_versioned_deferred_journal(
        &self,
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
    ) -> Result<Option<Vec<EntityKinematics>>, RegionOwnerLaneError> {
        self.try_apply_kinematics_if_versioned_deferred_journal_with_fence(expected, states)
            .map(|committed| committed.map(VersionedKinematicsCommit::into_states))
    }

    pub fn try_apply_kinematics_if_versioned_deferred_journal_with_fence(
        &self,
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
    ) -> Result<Option<VersionedKinematicsCommit>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyKinematicsIfVersioned {
                expected,
                states,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn try_apply_entity_physics_from_selected_routes(
        &self,
        expected: &VersionedEntityKinematics,
        queries: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
        publish_ids: &HashSet<EntityId>,
    ) -> Result<Option<SelectedPhysicsCommit>, RegionOwnerLaneError> {
        self.ensure_operational()?;
        match self.try_apply_entity_physics_from_selected_routes_inner(
            expected,
            queries,
            steps,
            publish_ids,
        ) {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    fn try_apply_entity_physics_from_selected_routes_inner(
        &self,
        expected: &VersionedEntityKinematics,
        queries: &[EntityPhysicsQuery],
        steps: &[EntityPhysicsStep],
        publish_ids: &HashSet<EntityId>,
    ) -> Option<Result<SelectedPhysicsCommit, RegionOwnerLaneError>> {
        if expected.authority != self.authority
            || expected.expected.len() != queries.len()
            || queries.len() != steps.len()
            || expected.routes.len() != queries.len()
            || expected.expected.len() != expected.routes.len()
        {
            return None;
        }
        let owner_readers = expected.owner_readers.as_ref()?;
        let _mutation_gate =
            read_authoritative_rwlock(&self.mutation_gate, "regional.mutation_gate");
        let mut lanes = BTreeMap::<
            usize,
            (
                RegionalOwnerLaneReader,
                BTreeMap<RegionKey, LocalPhysicsRegionBatch>,
            ),
        >::new();
        for (((previous, lease), query), step) in expected
            .expected
            .iter()
            .copied()
            .zip(expected.routes.iter().copied())
            .zip(queries.iter().copied())
            .zip(steps.iter().copied())
        {
            if previous.id != query.id
                || query.id != step.id
                || !previous.eligible()
                || previous.expected.position != query.position
                || previous.expected.velocity != query.velocity
                || previous.expected.on_ground != query.on_ground
                || !step.position.is_finite()
                || !step.velocity.is_finite()
                || RegionKey::from_position(query.position) != Some(lease.key)
                || RegionKey::from_position(step.position) != Some(lease.key)
            {
                return None;
            }
            let needs_region = lanes
                .get(&lease.lane)
                .is_none_or(|(_, regions)| !regions.contains_key(&lease.key));
            if needs_region {
                let owner = owner_readers.get(&lease.lane)?.clone();
                lanes
                    .entry(lease.lane)
                    .or_insert_with(|| (owner, BTreeMap::new()))
                    .1
                    .insert(
                        lease.key,
                        LocalPhysicsRegionBatch {
                            lease,
                            inputs: Vec::new(),
                            world_fence: None,
                        },
                    );
            }
            let (_, regions) = lanes.get_mut(&lease.lane)?;
            let batch = regions
                .get_mut(&lease.key)
                .expect("validated physics region batch");
            batch.inputs.push(LocalPhysicsInput {
                previous: previous.into(),
                expected: query,
                step,
                publish: !publish_ids.is_empty() && publish_ids.contains(&query.id),
            });
        }
        if lanes.len() != expected.fence.lane_versions.len() {
            return None;
        }
        let owners = lanes
            .iter()
            .map(|(lane, (owner, _))| (*lane, owner.clone()))
            .collect::<Vec<_>>();
        let batches = lanes
            .into_iter()
            .map(|(lane, (_, regions))| (lane, regions.into_values().collect::<Vec<_>>()))
            .collect::<Vec<_>>();
        let _lane_admissions = owners
            .iter()
            .map(|(_, owner)| {
                lock_authoritative_mutex(owner.admission(), "regional.owner_lane_admission")
            })
            .collect::<Vec<_>>();
        let mut pending = Vec::with_capacity(owners.len());
        let mut immediate_rejected = Vec::new();
        for ((lane, owner), (batch_lane, batches)) in owners.iter().zip(batches) {
            debug_assert_eq!(*lane, batch_lane);
            let expected_state_version = expected.fence.lane_versions.get(lane).copied()?;
            let lane_ids = batches
                .iter()
                .flat_map(|batch| batch.inputs.iter().map(|input| input.previous.id))
                .collect::<Vec<_>>();
            match owner.apply_local_physics_if_current(expected_state_version, batches) {
                Ok(completion) => {
                    pending.push((*lane, owner.clone(), lane_ids, completion));
                }
                Err(error) if self.local_physics_fallback_error(error) => {
                    immediate_rejected.extend(lane_ids);
                }
                Err(error) => return Some(Err(error)),
            }
        }
        Some((|| {
            let mut rejected = immediate_rejected;
            let mut accepted = Vec::new();
            let mut states = Vec::new();
            let mut leases = HashMap::new();
            let mut lane_versions = BTreeMap::new();
            for (lane, owner, lane_ids, completion) in pending {
                let completion = match completion.recv() {
                    Ok(Ok(completion)) => completion,
                    Ok(Err(error)) if self.local_physics_fallback_error(error) => {
                        rejected.extend(lane_ids);
                        continue;
                    }
                    Ok(Err(error)) => return Err(error),
                    Err(_) => {
                        let error = owner.unavailable_error();
                        if self.local_physics_fallback_error(error) {
                            rejected.extend(lane_ids);
                            continue;
                        }
                        return Err(error);
                    }
                };
                if !completion.accepted.is_empty() {
                    lane_versions.insert(lane, completion.state_version);
                }
                rejected.extend(completion.rejected);
                for (lease, motion) in completion.accepted {
                    if lease.lane != lane
                        || RegionKey::from_position(motion.position) != Some(lease.key)
                    {
                        return Err(RegionOwnerLaneError::InvalidQuery);
                    }
                    leases.insert(motion.id, lease);
                    accepted.push(motion);
                }
                states.extend(completion.states);
            }
            rejected.sort_unstable();
            accepted.sort_unstable_by_key(|motion| motion.id);
            states.sort_unstable_by_key(|state| state.id);
            Ok((
                rejected,
                accepted,
                states,
                VersionedEntitySnapshots {
                    authority: self.authority,
                    fence: RegionalVersionFence {
                        leases,
                        lane_versions,
                    },
                    snapshots: Vec::new(),
                    publication_readers: None,
                },
            ))
        })())
    }

    /// Attempts the cached multi-lane deferred-journal kinematics fast path and
    /// returns its committed snapshots with a version fence. This never falls
    /// back or fetches snapshots: region crossings, incomplete cached routes,
    /// and `max_workers <= 1` return `Ok(None)`.
    pub fn try_apply_kinematics_if_current_parallel_deferred_journal_versioned(
        &self,
        states: &[(EntitySnapshot, EntityKinematics)],
        max_workers: usize,
    ) -> Result<Option<VersionedEntitySnapshots>, RegionOwnerLaneError> {
        if max_workers <= 1 {
            return Ok(None);
        }
        if states.is_empty() {
            return Ok(Some(VersionedEntitySnapshots {
                authority: self.authority,
                fence: RegionalVersionFence {
                    leases: HashMap::new(),
                    lane_versions: BTreeMap::new(),
                },
                snapshots: Vec::new(),
                publication_readers: None,
            }));
        }
        let mut ids = HashSet::with_capacity(states.len());
        let mut direct_eligible = true;
        for (expected, state) in states {
            let source = RegionKey::from_position(expected.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if expected.id != state.id || !state.is_finite() || !ids.insert(expected.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if expected.retained.item_pickup_claim.is_some() && state.position != expected.position
            {
                return Ok(Some(VersionedEntitySnapshots {
                    authority: self.authority,
                    fence: RegionalVersionFence {
                        leases: HashMap::new(),
                        lane_versions: BTreeMap::new(),
                    },
                    snapshots: Vec::new(),
                    publication_readers: None,
                }));
            }
            direct_eligible &= RegionKey::from_position(state.position) == Some(source);
        }
        if !direct_eligible {
            return Ok(None);
        }
        let cached =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut groups = BTreeMap::<usize, Vec<(EntitySnapshot, EntityKinematics)>>::new();
        let mut routes_complete = true;
        for (expected, state) in states {
            let source = RegionKey::from_position(expected.position);
            let Some(route) = cached.get(&expected.id).filter(|route| {
                route.uuid == expected.uuid
                    && route.standalone
                    && source == Some(route.lease.key)
                    && RegionKey::from_position(state.position) == source
            }) else {
                routes_complete = false;
                break;
            };
            groups
                .entry(route.lease.lane)
                .or_default()
                .push((expected.clone(), *state));
        }
        drop(cached);
        if !routes_complete {
            return Ok(None);
        }

        let mut lane_mutations = Vec::with_capacity(groups.len());
        for (lane, group) in groups {
            lane_mutations.push((lane, kinematics_direct_mutations(&group)?));
        }
        match self.try_commit_multi_lane_cached_standalone_snapshot_mutations_deferred_journal(
            lane_mutations,
            max_workers,
        ) {
            Some(result) => result.map(Some),
            None => Ok(None),
        }
    }

    /// Conditional kinematics batch committed through cached owner-lane routes
    /// with lane work spread over at most `max_workers` scoped workers. Fast
    /// path semantics match the versioned method above; unavailable fast-path
    /// cases fall back to the existing serial deferred-journal path.
    pub fn apply_kinematics_if_current_parallel_deferred_journal_committed(
        &self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        max_workers: usize,
    ) -> Result<Vec<EntityKinematics>, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if let Some(selected) = self
            .try_apply_kinematics_if_current_parallel_deferred_journal_versioned(
                &states,
                max_workers,
            )?
        {
            return Ok(selected
                .into_snapshots()
                .into_iter()
                .map(snapshot_kinematics)
                .collect());
        }
        self.apply_kinematics_if_current_inner(states, false)
    }

    pub fn damage_if_current(
        &self,
        expected: EntitySnapshot,
        request: EntityDamageRequest,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        if !request.is_valid() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if expected.retained.item_pickup_claim.is_some() {
            return Ok(None);
        }
        if let Some(result) =
            self.try_commit_cached_snapshot_mutation(expected.clone(), |expected| {
                RegionOwnerMutation::DamageIfCurrent {
                    expected: Box::new(expected),
                    request,
                }
            })
        {
            return match result?.as_slice() {
                [] => Ok(None),
                [snapshot] => Ok(Some(EntityDamage {
                    killed: snapshot.lifecycle == crate::EntityLifecycle::Despawning,
                    snapshot: snapshot.clone(),
                })),
                _ => Err(RegionOwnerLaneError::InvalidMutation),
            };
        }

        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::DamageIfCurrent {
                expected: Box::new(expected),
                request,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn damage_batch_if_current(
        &self,
        requests: impl IntoIterator<Item = (EntitySnapshot, EntityDamageRequest)>,
    ) -> Result<Vec<EntityDamage>, RegionOwnerLaneError> {
        let requests = requests.into_iter().collect::<Vec<_>>();
        if requests.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = HashSet::with_capacity(requests.len());
        if requests.iter().any(|(expected, request)| {
            !request.is_valid()
                || expected.lifecycle != crate::EntityLifecycle::Alive
                || expected.retained.item_pickup_claim.is_some()
                || !ids.insert(expected.id)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let cached =
            read_benign_rwlock(&self.selected_read_routes, "regional.selected_read_routes");
        let mut groups = BTreeMap::<usize, Vec<(EntitySnapshot, EntityDamageRequest)>>::new();
        let mut routes_complete = true;
        for (expected, request) in requests {
            let route = cached.get(&expected.id).filter(|route| {
                route.uuid == expected.uuid
                    && RegionKey::from_position(expected.position) == Some(route.lease.key)
            });
            let Some(route) = route else {
                routes_complete = false;
                groups
                    .entry(usize::MAX)
                    .or_default()
                    .push((expected, request));
                continue;
            };
            groups
                .entry(route.lease.lane)
                .or_default()
                .push((expected, request));
        }
        drop(cached);

        if !routes_complete {
            let requests = groups.into_values().flatten().collect::<Vec<_>>();
            return requests
                .into_iter()
                .filter_map(|(expected, request)| {
                    self.damage_if_current(expected, request).transpose()
                })
                .collect();
        }

        let batches = std::thread::scope(|scope| {
            let mut workers = Vec::with_capacity(groups.len());
            for requests in groups.into_values() {
                let handle = self.clone();
                workers.push(scope.spawn(move || {
                    let mutations = requests
                        .iter()
                        .cloned()
                        .map(|(expected, request)| {
                            (
                                expected.clone(),
                                RegionOwnerMutation::DamageIfCurrent {
                                    expected: Box::new(expected),
                                    request,
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    match handle.try_commit_cached_snapshot_mutations(mutations) {
                        Some(result) => result,
                        None => requests
                            .into_iter()
                            .filter_map(|(expected, request)| {
                                handle.damage_if_current(expected, request).transpose()
                            })
                            .collect::<Result<Vec<_>, _>>()
                            .map(|damages| {
                                damages.into_iter().map(|damage| damage.snapshot).collect()
                            }),
                    }
                }));
            }
            workers
                .into_iter()
                .map(|worker| worker.join().map_err(|_| self.unavailable_error())?)
                .collect::<Result<Vec<_>, _>>()
        })?;

        let mut damages = batches
            .into_iter()
            .flatten()
            .map(|snapshot| EntityDamage {
                killed: snapshot.lifecycle == crate::EntityLifecycle::Despawning,
                snapshot,
            })
            .collect::<Vec<_>>();
        damages.sort_unstable_by_key(|damage| damage.snapshot.id);
        Ok(damages)
    }

    pub fn apply_effect_if_current(
        &self,
        expected: EntitySnapshot,
        request: EntityEffectRequest,
    ) -> Result<EntityEffectResult, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyEffectIfCurrent {
                expected: Box::new(expected),
                request: Box::new(request),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn damage(
        &self,
        entity: EntityId,
        request: EntityDamageRequest,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::Damage {
                entity,
                request,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn tick_owned_regions(
        &self,
        input: RegionalEntityTickInput,
    ) -> Result<RegionalEntityTickOutput, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::TickOwnedRegions { input, reply })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn commit_owned_region_physics(
        &self,
        prepared: RegionalPreparedEntityPhysics,
    ) -> Result<RegionalEntityPhysicsOutput, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::CommitOwnedRegionPhysics {
                prepared: Box::new(prepared),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &self,
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_pathing_for_ids_with_inputs(
            tick,
            active_ids,
            RegionalGoalTickInputs::default(),
        )
    }

    pub fn prepare_goal_tick_with_pathing_for_ids_with_inputs(
        &self,
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
        inputs: RegionalGoalTickInputs,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_optional_snapshots(tick, active_ids, inputs, None)
    }

    pub fn prepare_goal_tick_with_pathing_for_versioned_snapshots(
        &self,
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
        selected: VersionedEntitySnapshots,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_pathing_for_versioned_snapshots_with_inputs(
            tick,
            active_ids,
            selected,
            RegionalGoalTickInputs::default(),
        )
    }

    pub fn prepare_goal_tick_with_pathing_for_versioned_snapshots_with_inputs(
        &self,
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
        selected: VersionedEntitySnapshots,
        inputs: RegionalGoalTickInputs,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_optional_snapshots(tick, active_ids, inputs, Some(selected))
    }

    fn prepare_goal_tick_with_optional_snapshots(
        &self,
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
        inputs: RegionalGoalTickInputs,
        selected: Option<VersionedEntitySnapshots>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::PrepareGoalTick {
                tick,
                active_ids,
                selected,
                inputs,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn apply_prepared_goal_tick(
        &self,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTick {
                resolved: Box::new(resolved),
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }

    pub fn apply_prepared_goal_tick_and_kinematics_for_ids(
        &self,
        resolved: RegionalResolvedGoalTick,
        entities: Arc<HashSet<EntityId>>,
    ) -> GoalApplyKinematicsResult {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved: Box::new(resolved),
                entities,
                defer_journal: false,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result
            .recv()
            .map_err(|_| self.unavailable_error())?
            .map(|applied| {
                applied.map(|(stats, results, versioned)| {
                    let states = results
                        .into_iter()
                        .map(|result| EntityKinematics {
                            id: result.physics.id,
                            position: result.physics.position,
                            rotation: result.rotation,
                            velocity: result.physics.velocity,
                            on_ground: result.physics.on_ground,
                        })
                        .collect();
                    (stats, states, versioned)
                })
            })
    }

    pub fn apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(
        &self,
        resolved: RegionalResolvedGoalTick,
        entities: Arc<HashSet<EntityId>>,
    ) -> GoalApplyKinematicsResult {
        self.apply_prepared_goal_tick_and_simulation_results_for_ids_deferred_journal(
            resolved, entities,
        )
        .map(|applied| {
            applied.map(|(stats, results, versioned)| {
                let states = results
                    .into_iter()
                    .map(|result| EntityKinematics {
                        id: result.physics.id,
                        position: result.physics.position,
                        rotation: result.rotation,
                        velocity: result.physics.velocity,
                        on_ground: result.physics.on_ground,
                    })
                    .collect();
                (stats, states, versioned)
            })
        })
    }

    pub fn apply_prepared_goal_tick_and_simulation_results_for_ids_deferred_journal(
        &self,
        resolved: RegionalResolvedGoalTick,
        entities: Arc<HashSet<EntityId>>,
    ) -> GoalApplySimulationResult {
        let (reply, result) = channel();
        self.sender
            .send(RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved: Box::new(resolved),
                entities,
                defer_journal: true,
                reply,
            })
            .map_err(|_| self.unavailable_error())?;
        result.recv().map_err(|_| self.unavailable_error())?
    }
}
