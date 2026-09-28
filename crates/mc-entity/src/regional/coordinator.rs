use super::*;

impl RegionalOwnerCoordinator {
    pub fn from_store(
        regions: RegionalEntityStore,
        requested_lanes: usize,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        Self::from_store_with_journal(
            regions,
            requested_lanes,
            Box::new(NoopRegionalDecisionJournal),
        )
    }

    pub fn from_store_with_journal(
        regions: RegionalEntityStore,
        requested_lanes: usize,
        journal: Box<dyn RegionalDecisionJournal>,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        if requested_lanes == 0 {
            return Err(RegionalOwnerCutoverError {
                error: RegionOwnerLaneError::InvalidLaneCount,
                store: Box::new(regions),
            });
        }
        if regions.ownership.active_phase.is_some()
            || !regions.transfers.is_empty()
            || !regions.in_flight_transfers.is_empty()
        {
            return Err(RegionalOwnerCutoverError {
                error: RegionOwnerLaneError::Busy,
                store: Box::new(regions),
            });
        }
        let vehicle_links = regions
            .stores
            .values()
            .flat_map(EntityStore::snapshots)
            .filter_map(|snapshot| {
                snapshot_vehicle_reference(&snapshot).map(|passenger| (snapshot.id, passenger))
            })
            .collect::<Vec<_>>();
        let mut vehicle_passengers = HashMap::new();
        let mut passenger_vehicles = HashMap::new();
        for (vehicle, passenger) in vehicle_links {
            if !regions.locations.contains_key(&passenger)
                || vehicle_passengers.insert(vehicle, passenger).is_some()
                || passenger_vehicles.insert(passenger, vehicle).is_some()
            {
                return Err(RegionalOwnerCutoverError {
                    error: RegionOwnerLaneError::InvalidMutation,
                    store: Box::new(regions),
                });
            }
        }
        let RegionalEntityStore {
            authority,
            mut ownership,
            mut stores,
            mut locations,
            mut uuids,
            transfers,
            in_flight_transfers,

            next_id,
        } = regions;
        let (recovered_phase, recovered_sequence) = journal.recovery_watermark();
        let recovered_lifecycle_epoch = journal
            .pending_commit_identities()
            .into_iter()
            .map(|identity| identity.2)
            .max()
            .unwrap_or(0);
        ownership.last_phase = ownership.last_phase.max(recovered_phase.0);
        let lane_count = requested_lanes;
        let mut by_lane = (0..lane_count)
            .map(|lane| (lane, Vec::new()))
            .collect::<BTreeMap<_, _>>();
        let keys = stores.keys().copied().collect::<Vec<_>>();
        for (index, key) in keys.into_iter().enumerate() {
            let target_lane = index % lane_count;
            let current = ownership.lease(key).expect("owned store has a lease");
            let lease = if current.lane == target_lane {
                current
            } else {
                ownership
                    .reassign(current, target_lane)
                    .expect("inactive owner cutover reassigns exact lease")
            };
            let store = stores.remove(&key).expect("known owner cutover store");
            by_lane
                .get_mut(&target_lane)
                .expect("known owner lane")
                .push((lease, store));
        }
        let mut lanes = BTreeMap::new();
        while let Some((lane, stores)) = by_lane.pop_first() {
            match RegionalOwnerLane::spawn_after(lane, stores, recovered_phase, recovered_sequence)
            {
                Ok(owner) => {
                    lanes.insert(lane, owner);
                }
                Err(start) => {
                    let mut recovered = start
                        .regions
                        .into_iter()
                        .map(|(lease, store)| (lease.key, store))
                        .collect::<BTreeMap<_, _>>();
                    for (_, pending) in by_lane {
                        recovered
                            .extend(pending.into_iter().map(|(lease, store)| (lease.key, store)));
                    }
                    let mut recovery_error = start.error;
                    for (_, owner) in lanes {
                        match owner.shutdown() {
                            Ok(stores) => recovered.extend(stores),
                            Err(error) => recovery_error = error,
                        }
                    }
                    retain_recovered_regional_state(
                        &mut ownership,
                        &recovered,
                        &mut locations,
                        &mut uuids,
                    );
                    return Err(RegionalOwnerCutoverError {
                        error: recovery_error,
                        store: Box::new(RegionalEntityStore {
                            authority,
                            ownership,
                            stores: recovered,
                            locations,
                            uuids,
                            transfers,
                            in_flight_transfers,

                            next_id,
                        }),
                    });
                }
            }
        }
        let journal_commits = journal
            .pending_commit_identities()
            .into_iter()
            .map(|identity| (identity.0, identity))
            .collect::<BTreeMap<_, _>>();
        let commit_state = Arc::new(RegionalOwnerCommitState {
            next_phase: AtomicU64::new(ownership.last_phase.max(recovered_phase.0)),
            next_sequence: AtomicU64::new(recovered_sequence),
            lifecycle_epoch: AtomicU64::new(recovered_lifecycle_epoch),
            journal: Mutex::new(journal),
            journal_commits: Mutex::new(journal_commits),
            outcome_unknown: AtomicBool::new(false),
            #[cfg(test)]
            save_barrier_phase_snapshot_hook: Mutex::new(None),
        });
        Ok(Self {
            authority,
            ownership,
            locations,
            uuids,
            vehicle_passengers,
            passenger_vehicles,
            transfers,
            in_flight_transfers,
            next_id,
            lanes,
            commit_state,
        })
    }

    #[must_use]
    pub fn lane_count(&self) -> usize {
        self.lanes.len()
    }

    pub fn reconfigure_lanes(
        &mut self,
        requested_lanes: usize,
    ) -> Result<usize, RegionOwnerLaneError> {
        if requested_lanes == 0 {
            return Err(RegionOwnerLaneError::InvalidLaneCount);
        }

        for lane in 0..requested_lanes {
            if self.lanes.contains_key(&lane) {
                continue;
            }
            let owner = RegionalOwnerLane::spawn_after(
                lane,
                [],
                RegionPhase(self.commit_state.next_phase.load(Ordering::Acquire)),
                self.commit_state.sequence_watermark(),
            )
            .map_err(|start| start.error)?;
            self.lanes.insert(lane, owner);
        }

        let leases = self.ownership.leases().collect::<Vec<_>>();
        for (index, lease) in leases.into_iter().enumerate() {
            let target_lane = index % requested_lanes;
            if lease.lane != target_lane {
                self.move_region(lease, target_lane)?;
            }
        }

        let retiring = self
            .lanes
            .range(requested_lanes..)
            .map(|(&lane, _)| lane)
            .collect::<Vec<_>>();
        for lane in retiring {
            let owner = self
                .lanes
                .remove(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let stores = owner.shutdown()?;
            if !stores.is_empty() {
                let regions = stores
                    .into_iter()
                    .map(|(key, store)| {
                        let lease = self
                            .ownership
                            .lease(key)
                            .expect("retiring owner region retains its lease");
                        (lease, store)
                    })
                    .collect::<Vec<_>>();
                let restored = RegionalOwnerLane::spawn_after(
                    lane,
                    regions,
                    RegionPhase(self.commit_state.next_phase.load(Ordering::Acquire)),
                    self.commit_state.sequence_watermark(),
                )
                .map_err(|start| start.error)?;
                self.lanes.insert(lane, restored);
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        Ok(self.lanes.len())
    }

    pub fn clear_recovered_commits(
        &mut self,
        phases: impl IntoIterator<Item = RegionPhase>,
    ) -> Result<(), RegionOwnerLaneError> {
        let phases = phases.into_iter().collect::<Vec<_>>();
        self.commit_state.clear_recovered_commits(&phases)
    }

    fn move_region(
        &mut self,
        expected: RegionLease,
        target_lane: usize,
    ) -> Result<RegionLease, RegionOwnerLaneError> {
        if !self.lanes.contains_key(&target_lane) {
            return Err(RegionOwnerLaneError::WrongLane);
        }
        let (_, store) = self
            .lanes
            .get(&expected.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .detach_region(expected)?;
        let reassigned = match self.ownership.reassign(expected, target_lane) {
            Ok(reassigned) => reassigned,
            Err(_) => {
                let rollback = self
                    .lanes
                    .get(&expected.lane)
                    .expect("source lane was validated before detach")
                    .install_region(expected, store);
                if let Err(failure) = rollback {
                    self.commit_state.mark_outcome_unknown();
                    return Err(failure.error);
                }
                return Err(RegionOwnerLaneError::Busy);
            }
        };
        let installed = self
            .lanes
            .get(&target_lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .install_region(reassigned, store);
        if let Err(failure) = installed {
            let error = failure.error;
            let Some(store) = failure.recovered else {
                self.commit_state.mark_outcome_unknown();
                return Err(error);
            };
            let restored = self
                .ownership
                .reassign(reassigned, expected.lane)
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
            if let Err(rollback) = self
                .lanes
                .get(&expected.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .install_region(restored, *store)
            {
                self.commit_state.mark_outcome_unknown();
                return Err(rollback.error);
            }
            return Err(error);
        }
        Ok(reassigned)
    }

    pub fn snapshot(
        &self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let Some(key) = self.locations.get(&entity).copied() else {
            return Ok(None);
        };
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        self.lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .snapshot(lease, entity)
    }

    pub fn snapshots(&self) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let mut pending = Vec::with_capacity(self.lanes.len());
        for owner in self.lanes.values() {
            pending.push(owner.request_snapshots()?);
        }
        let mut snapshots = Vec::with_capacity(self.locations.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)?,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        if snapshots.len() != self.locations.len()
            || snapshots.iter().any(|snapshot| {
                self.locations.get(&snapshot.id).copied()
                    != RegionKey::from_position(snapshot.position)
                    || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(snapshots)
    }

    fn fenced_simulation_results_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<FencedSimulationResults, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let Some(expected_key) = self.locations.get(&entity).copied() else {
                continue;
            };
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push((
                lane,
                owner.reader().request_alive_kinematics_for_ids(entities)?,
            ));
        }
        let mut candidate_runs = Vec::with_capacity(pending.len());
        let mut motion_states = Vec::new();
        let mut lane_versions = BTreeMap::new();
        for (expected_lane, completion) in pending {
            let read = completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??;
            if read.lane != expected_lane
                || lane_versions
                    .insert(read.lane, read.state_version)
                    .is_some()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            candidate_runs.push(read.candidates);
            motion_states.extend(read.motion_states);
        }
        let candidates = owner_lane::merge_fenced_kinematics_candidate_runs(candidate_runs);
        motion_states.sort_unstable_by_key(|motion| motion.id);
        self.fenced_simulation_results_from_candidates(
            entities,
            candidates,
            lane_versions,
            Some(motion_states),
            false,
        )
    }

    fn fenced_simulation_results_from_candidates(
        &self,
        entities: &HashSet<EntityId>,
        candidates: Vec<FencedKinematicsCandidate>,
        lane_versions: BTreeMap<usize, u64>,
        motion_states: Option<Vec<EntityMotionState>>,
        selection_is_exact: bool,
    ) -> Result<FencedSimulationResults, RegionOwnerLaneError> {
        self.fenced_simulation_results_from_candidate_runs(
            entities,
            vec![candidates],
            lane_versions,
            motion_states,
            selection_is_exact,
        )
    }

    fn fenced_simulation_results_from_candidate_runs(
        &self,
        entities: &HashSet<EntityId>,
        mut candidate_runs: Vec<Vec<FencedKinematicsCandidate>>,
        mut lane_versions: BTreeMap<usize, u64>,
        motion_states: Option<Vec<EntityMotionState>>,
        selection_is_exact: bool,
    ) -> Result<FencedSimulationResults, RegionOwnerLaneError> {
        if selection_is_exact {
            // The goal token was already checked for exact set equality before
            // owner commit, and each candidate ID was checked against its
            // regional store during owner preflight.
            if candidate_runs.iter().map(Vec::len).sum::<usize>() != entities.len() {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        } else {
            for candidates in &mut candidate_runs {
                candidates.retain(|candidate| entities.contains(&candidate.id));
            }
        }
        let candidate_count = candidate_runs.iter().map(Vec::len).sum::<usize>();
        let motion_states = motion_states.map(|mut motion_states| {
            motion_states.retain(|motion| entities.contains(&motion.id));
            motion_states
        });
        if motion_states
            .as_ref()
            .is_some_and(|motion_states| motion_states.len() != candidate_count)
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let vehicle_free = self.vehicle_passengers.is_empty() && self.passenger_vehicles.is_empty();
        let mut previous_id = None;
        let mut eligible = true;
        let mut results = Vec::with_capacity(candidate_count);
        let mut routes = Vec::with_capacity(candidate_count);
        let mut expected = Vec::with_capacity(candidate_count);
        let mut selected_lanes = BTreeSet::new();
        for (index, candidate) in candidate_runs.into_iter().flatten().enumerate() {
            if (!selection_is_exact && previous_id.is_some_and(|previous| previous >= candidate.id))
                || motion_states
                    .as_ref()
                    .is_some_and(|states| states[index].id != candidate.id)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            previous_id = Some(candidate.id);
            eligible &= candidate.eligible()
                && (vehicle_free
                    || (!self.vehicle_passengers.contains_key(&candidate.id)
                        && !self.passenger_vehicles.contains_key(&candidate.id)));
            if let Some(result) = candidate.result {
                results.push(result);
            }
            if let Some(checkpoint) = candidate.expected() {
                expected.push(checkpoint);
            }
            selected_lanes.insert(candidate.lease.lane);
            routes.push(candidate.lease);
        }

        let states = motion_states
            .as_ref()
            .map_or_else(Vec::new, |motion_states| {
                motion_states
                    .iter()
                    .map(|motion| EntityKinematics {
                        id: motion.id,
                        position: motion.position,
                        rotation: motion.rotation,
                        velocity: motion.velocity,
                        on_ground: motion.on_ground,
                    })
                    .collect()
            });
        let versioned = if eligible {
            for (result, lease) in results.iter().zip(&routes) {
                if RegionKey::from_position(result.physics.position) != Some(lease.key)
                    || !lane_versions.contains_key(&lease.lane)
                {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
            }
            lane_versions.retain(|lane, _| selected_lanes.contains(lane));
            if !selected_lanes.iter().eq(lane_versions.keys()) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let owner_readers = Arc::new(
                selected_lanes
                    .iter()
                    .map(|lane| {
                        self.lanes
                            .get(lane)
                            .map(|owner| (*lane, owner.reader()))
                            .ok_or(RegionOwnerLaneError::WrongLane)
                    })
                    .collect::<Result<BTreeMap<_, _>, _>>()?,
            );
            Some(VersionedEntityKinematics {
                authority: self.authority,
                fence: RegionalVersionFence {
                    leases: HashMap::new(),
                    lane_versions,
                },
                routes,
                motion_states,
                expected,
                owner_readers: Some(owner_readers),
            })
        } else {
            None
        };
        Ok((states, results, versioned))
    }

    pub(super) fn alive_kinematics_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<(Vec<EntityKinematics>, Option<VersionedEntityKinematics>), RegionOwnerLaneError>
    {
        let (states, _, versioned) = self.fenced_simulation_results_for_ids(entities)?;
        Ok((states, versioned))
    }

    pub(super) fn simulation_results_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<
        (
            Vec<EntitySimulationResult>,
            Option<VersionedEntityKinematics>,
        ),
        RegionOwnerLaneError,
    > {
        let (_, results, versioned) = self.fenced_simulation_results_for_ids(entities)?;
        Ok((results, versioned))
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.uuids.contains_key(&uuid)
    }

    /// Resolve bounded entity UUIDs to their current snapshots in caller order.
    pub fn entities_by_uuid(
        &self,
        uuids: &[Uuid],
    ) -> Result<Vec<Option<EntitySnapshot>>, RegionOwnerLaneError> {
        uuids
            .iter()
            .map(|uuid| match self.uuids.get(uuid).copied() {
                Some(entity) => self.snapshot(entity),
                None => Ok(None),
            })
            .collect()
    }

    fn goal_checkpoints_for_ids_fenced(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<(Vec<EntityGoalCheckpoint>, BTreeMap<usize, u64>), RegionOwnerLaneError> {
        self.goal_checkpoints_for_ids_inner(entities, false)
    }

    fn goal_checkpoints_for_ids_admitted(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<(Vec<EntityGoalCheckpoint>, BTreeMap<usize, u64>), RegionOwnerLaneError> {
        self.goal_checkpoints_for_ids_inner(entities, true)
    }

    fn goal_checkpoints_for_ids_inner(
        &self,
        entities: &HashSet<EntityId>,
        acquire_admission: bool,
    ) -> Result<(Vec<EntityGoalCheckpoint>, BTreeMap<usize, u64>), RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let expected_key = self
                .locations
                .get(&entity)
                .copied()
                .ok_or(RegionOwnerLaneError::UnknownEntity)?;
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        let mut lane_versions = BTreeMap::new();
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let completion = if acquire_admission {
                owner.request_goal_checkpoints_for_ids_admitted(entities)?
            } else {
                owner.request_goal_checkpoints_for_ids_fenced(entities)?
            };
            lane_versions.insert(lane, owner.reader().state_version());
            pending.push((lane, completion));
        }
        let mut checkpoints = Vec::with_capacity(entities.len());
        for (_, completion) in pending {
            checkpoints.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        checkpoints.sort_unstable_by_key(|checkpoint| checkpoint.id);
        if checkpoints.len() != entities.len()
            || checkpoints.iter().any(|checkpoint| {
                !entities.contains(&checkpoint.id)
                    || self.locations.get(&checkpoint.id).copied()
                        != RegionKey::from_position(checkpoint.position)
            })
            || lane_versions.iter().any(|(lane, version)| {
                self.lanes
                    .get(lane)
                    .is_none_or(|owner| owner.reader().state_version() != *version)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok((checkpoints, lane_versions))
    }

    pub fn snapshots_for_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.snapshots_for_ids_selected(entities, &SnapshotSelection::All)
    }

    pub(super) fn snapshots_for_ids_selected(
        &self,
        entities: &HashSet<EntityId>,
        selection: &SnapshotSelection,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let mut ordered = entities.iter().copied().collect::<Vec<_>>();
        ordered.sort_unstable();
        let mut requests = BTreeMap::<usize, Vec<(RegionLease, EntityId)>>::new();
        for entity in ordered {
            let Some(expected_key) = self.locations.get(&entity).copied() else {
                continue;
            };
            let lease = self
                .ownership
                .lease(expected_key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            requests
                .entry(lease.lane)
                .or_default()
                .push((lease, entity));
        }
        let mut pending = Vec::with_capacity(requests.len());
        for (lane, entities) in requests {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push(owner.request_snapshots_for_ids(entities, selection.clone())?);
        }
        let mut snapshots = Vec::with_capacity(entities.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        for snapshot in &snapshots {
            let expected_key = self
                .locations
                .get(&snapshot.id)
                .copied()
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if RegionKey::from_position(snapshot.position) != Some(expected_key)
                || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        Ok(snapshots)
    }

    #[must_use]
    pub fn status(&self) -> RegionalOwnerStatus {
        RegionalOwnerStatus {
            entity_count: self.locations.len(),
            lane_count: self.lanes.len(),
        }
    }

    pub fn save_barrier(&self) -> Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        let sequence_watermark = self.commit_state.sequence_watermark();
        let lifecycle_epoch = self.commit_state.lifecycle_epoch();
        let mut leases_by_lane = BTreeMap::<usize, Vec<RegionLease>>::new();
        for lease in self.ownership.leases() {
            leases_by_lane.entry(lease.lane).or_default().push(lease);
        }
        let mut pending = Vec::with_capacity(leases_by_lane.len());
        for (lane, leases) in leases_by_lane {
            let completion = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .request_save_barrier(sequence_watermark, leases)?;
            pending.push(completion);
        }
        let mut snapshots = Vec::with_capacity(self.locations.len());
        for completion in pending {
            snapshots.extend(
                completion
                    .recv()
                    .map_err(|_| RegionOwnerLaneError::Closed)??,
            );
        }
        snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
        if snapshots.len() != self.locations.len()
            || snapshots.iter().any(|snapshot| {
                self.locations.get(&snapshot.id).copied()
                    != RegionKey::from_position(snapshot.position)
                    || self.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        Ok(RegionalOwnerSaveSnapshot {
            sequence_watermark,
            lifecycle_epoch,
            snapshots,
            journal_phases: self.commit_state.pending_journal_phases(sequence_watermark),
        })
    }

    pub fn insert_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionOwnerLaneError> {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        if snapshots.is_empty() {
            return Ok(0);
        }
        let mut pending_ids = HashSet::with_capacity(snapshots.len());
        let mut pending_uuids = HashSet::with_capacity(snapshots.len());
        let mut locations = self.locations.clone();
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if !snapshot.rotation.is_finite()
                || !snapshot.velocity.is_finite()
                || self.locations.contains_key(&snapshot.id)
                || self.uuids.contains_key(&snapshot.uuid)
                || !pending_ids.insert(snapshot.id)
                || !pending_uuids.insert(snapshot.uuid)
                || locations.insert(snapshot.id, key).is_some()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        if snapshots.iter().any(|snapshot| {
            let key = locations[&snapshot.id];
            snapshot_vehicle_reference(snapshot)
                .is_some_and(|passenger| locations.get(&passenger).copied() != Some(key))
                || goal_reference(&snapshot.goal)
                    .is_some_and(|target| !locations.contains_key(&target))
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if snapshots
            .iter()
            .any(|snapshot| snapshot_vehicle_reference(snapshot).is_some())
        {
            let current = self.snapshots()?;
            let mut graph_validator = EntityStore::new();
            if !graph_validator.insert_snapshots_batch(current.into_iter().chain(snapshots.clone()))
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }

        let mut grouped = BTreeMap::<RegionKey, Vec<EntitySnapshot>>::new();
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .expect("validated owner restore position");
            self.ensure_region(key)?;
            grouped.entry(key).or_default().push(snapshot.clone());
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(grouped.len())?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (key, snapshots)) in grouped.into_iter().enumerate() {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::InsertSnapshots(snapshots),
                });
        }
        self.execute_mutations(mutations, next_sequence)?;
        for snapshot in &snapshots {
            let key = RegionKey::from_position(snapshot.position)
                .expect("validated owner restore position");
            self.next_id = self.next_id.max(snapshot.id.0);
            self.locations.insert(snapshot.id, key);
            self.uuids.insert(snapshot.uuid, snapshot.id);
            self.index_vehicle_link(snapshot);
        }
        Ok(snapshots.len())
    }

    pub fn spawn(&mut self, entity: SpawnEntity) -> Result<EntityId, RegionOwnerLaneError> {
        self.spawn_inner(entity, true)
    }

    pub(super) fn spawn_inner(
        &mut self,
        entity: SpawnEntity,
        journal_commit: bool,
    ) -> Result<EntityId, RegionOwnerLaneError> {
        let key = RegionKey::from_position(entity.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if !entity.rotation.is_finite()
            || !entity.velocity.is_finite()
            || entity
                .vehicle
                .as_ref()
                .and_then(|vehicle| vehicle.passenger)
                .is_some()
            || goal_reference(&entity.goal).is_some()
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let lease = self.ensure_region(key)?;
        let id = self.next_owner_id()?;
        let next_id_after_reservation =
            id.0.checked_add(i32::from(entity.reserved_following_ids))
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        if self.uuids.contains_key(&uuid) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let snapshot = snapshot_from_spawn(id, uuid, entity);
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::InsertSnapshot(Box::new(snapshot)),
            }],
        )]);
        self.execute_mutations_with_stats(mutations, sequence, journal_commit)?;
        self.next_id = next_id_after_reservation;
        self.locations.insert(id, key);
        self.uuids.insert(uuid, id);
        Ok(id)
    }

    pub fn spawn_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionOwnerLaneError> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entities.len());
        let mut ids = Vec::with_capacity(entities.len());
        let mut pending_ids = HashSet::with_capacity(entities.len());
        let mut pending_uuids = HashSet::with_capacity(entities.len());
        let mut cursor = self.next_id;
        for entity in entities {
            if RegionKey::from_position(entity.position).is_none()
                || !entity.rotation.is_finite()
                || !entity.velocity.is_finite()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let id = loop {
                cursor = cursor
                    .max(0)
                    .checked_add(1)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let id = EntityId(cursor);
                if !self.locations.contains_key(&id) && pending_ids.insert(id) {
                    break id;
                }
            };
            let reserved_following_ids = entity.reserved_following_ids;
            let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
            if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            ids.push(id);
            snapshots.push(snapshot_from_spawn(id, uuid, entity));
            cursor = cursor
                .checked_add(i32::from(reserved_following_ids))
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        }
        let expected = snapshots.len();
        if self.insert_snapshots_batch(snapshots)? != expected {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        self.next_id = self.next_id.max(cursor);
        Ok(ids)
    }

    pub fn spawn_unique_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntitySnapshot>, RegionOwnerLaneError> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        let mut snapshots = Vec::with_capacity(entities.len());
        let mut pending_ids = HashSet::with_capacity(entities.len());
        let mut pending_uuids = HashSet::with_capacity(entities.len());
        let mut cursor = self.next_id;
        for entity in entities {
            if let Some(uuid) = entity.uuid
                && (self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid))
            {
                continue;
            }
            if RegionKey::from_position(entity.position).is_none()
                || !entity.rotation.is_finite()
                || !entity.velocity.is_finite()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let id = loop {
                cursor = cursor
                    .max(0)
                    .checked_add(1)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let id = EntityId(cursor);
                if !self.locations.contains_key(&id) && pending_ids.insert(id) {
                    break id;
                }
            };
            let reserved_following_ids = entity.reserved_following_ids;
            let uuid = if let Some(uuid) = entity.uuid {
                uuid
            } else {
                let uuid = deterministic_uuid(id);
                if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                uuid
            };
            snapshots.push(snapshot_from_spawn(id, uuid, entity));
            cursor = cursor
                .checked_add(i32::from(reserved_following_ids))
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        }
        if snapshots.is_empty() {
            return Ok(Vec::new());
        }
        let expected = snapshots.len();
        if self.insert_snapshots_batch(snapshots.clone())? != expected {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        self.next_id = self.next_id.max(cursor);
        Ok(snapshots)
    }

    pub fn remove(
        &mut self,
        entity: EntityId,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let Some(key) = self.locations.get(&entity).copied() else {
            return Ok(None);
        };
        if self.in_flight_transfers.contains_key(&entity) {
            return Err(RegionOwnerLaneError::Busy);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let snapshot = self
            .lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .snapshot(lease, entity)?
            .ok_or(RegionOwnerLaneError::UnknownEntity)?;
        if snapshot.retained.item_pickup_claim.is_some() {
            return Ok(None);
        }
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::RemoveEntity(entity),
            }],
        )]);
        self.execute_mutations(mutations, sequence)?;
        self.remove_vehicle_links(&snapshot);
        self.locations.remove(&entity);
        self.uuids.remove(&snapshot.uuid);
        Ok(Some(snapshot))
    }

    pub fn remove_if_current(
        &mut self,
        expected: EntitySnapshot,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        if expected.retained.item_pickup_claim.is_some() {
            return Ok(None);
        }
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(None);
        }
        if self.in_flight_transfers.contains_key(&expected.id) {
            return Err(RegionOwnerLaneError::Busy);
        }
        if self.snapshot(expected.id)?.as_ref() != Some(&expected) {
            return Ok(None);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let entity = expected.id;
        let uuid = expected.uuid;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::RemoveIfCurrent(Box::new(expected.clone())),
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => {
                self.remove_vehicle_links(&expected);
                self.locations.remove(&entity);
                self.uuids.remove(&uuid);
                Ok(Some(expected))
            }
            Err(error) => Err(error),
        }
    }

    pub(super) fn prepare_snapshot_if_current(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<Option<(RegionPhase, usize, u64)>, RegionOwnerLaneError> {
        if expected.id != next.id
            || expected.uuid != next.uuid
            || expected.position != next.position
            || expected.type_id != next.type_id
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let key = RegionKey::from_position(expected.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if self.locations.get(&expected.id).copied() != Some(key)
            || self.in_flight_transfers.contains_key(&expected.id)
            || self.snapshot(expected.id)?.as_ref() != Some(&expected)
        {
            return Ok(None);
        }
        let (.., sequence) = self.commit_state.reserve_sequences(1)?;
        let phase = self.commit_state.reserve_phase()?;
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        self.ownership
            .begin_allocated_phase_for_lanes(phase, BTreeSet::from([lease.lane]))
            .map_err(|_| RegionOwnerLaneError::Busy)?;
        let prepared = self.lanes[&lease.lane].prepare(RegionOwnerBatch {
            phase,
            sequence_watermark: sequence,
            mutations: vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                    expected: Box::new(expected),
                    next: Box::new(next),
                    allow_type_change: false,
                },
            }],
        });
        let result = prepared.and_then(|completion| {
            completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)?
        });
        match result {
            Ok(ready) if ready == phase => Ok(Some((phase, lease.lane, sequence))),
            Ok(_) => {
                self.abort_prepared_snapshot(phase, lease.lane)?;
                Err(RegionOwnerLaneError::StalePhase)
            }
            Err(RegionOwnerLaneError::Closed) => {
                self.commit_state
                    .outcome_unknown
                    .store(true, Ordering::Release);
                Err(RegionOwnerLaneError::Closed)
            }
            Err(error) => {
                self.finish_prepared_snapshot(phase, lease.lane)?;
                Err(error)
            }
        }
    }

    fn finish_prepared_snapshot(
        &mut self,
        phase: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnerLaneError> {
        self.ownership
            .acknowledge_lane(phase, lane)
            .map_err(|_| RegionOwnerLaneError::StalePhase)?;
        self.ownership
            .finish_phase(phase)
            .map_err(|_| RegionOwnerLaneError::StalePhase)
    }

    pub(super) fn abort_prepared_snapshot(
        &mut self,
        phase: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnerLaneError> {
        if self.ownership.active_phase != Some(phase)
            || !self.ownership.pending_lanes.contains(&lane)
        {
            return Err(RegionOwnerLaneError::StalePhase);
        }
        let aborted = self.lanes[&lane]
            .abort(phase)?
            .recv()
            .map_err(|_| RegionOwnerLaneError::Closed)??;
        if aborted != phase {
            return Err(RegionOwnerLaneError::StalePhase);
        }
        self.finish_prepared_snapshot(phase, lane)
    }

    pub(super) fn commit_prepared_snapshot(
        &mut self,
        phase: RegionPhase,
        lane: usize,
        sequence: u64,
        entity: EntityId,
    ) -> Result<(), RegionOwnerLaneError> {
        if self.ownership.active_phase != Some(phase)
            || !self.ownership.pending_lanes.contains(&lane)
        {
            return Err(RegionOwnerLaneError::StalePhase);
        }
        let key = self
            .locations
            .get(&entity)
            .copied()
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        if lease.lane != lane {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        let committed = self.lanes[&lane]
            .commit(phase)?
            .recv()
            .map_err(|_| RegionOwnerLaneError::Closed)??;
        if committed.phase != phase {
            return Err(RegionOwnerLaneError::StalePhase);
        }
        if self
            .record_commit_decision(
                phase,
                sequence,
                &BTreeMap::from([(lane, vec![(lease, entity)])]),
            )
            .is_err()
        {
            let rollback = self.lanes[&lane]
                .rollback(phase)?
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??;
            if rollback != phase {
                return Err(RegionOwnerLaneError::StalePhase);
            }
            return Err(RegionOwnerLaneError::Journal);
        }
        let finalized = self.lanes[&lane]
            .finalize(phase)?
            .recv()
            .map_err(|_| RegionOwnerLaneError::Closed)??;
        if finalized != phase {
            return Err(RegionOwnerLaneError::StalePhase);
        }
        self.finish_prepared_snapshot(phase, lane)
    }

    pub fn replace_snapshot_if_current(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.replace_snapshot_if_current_inner(expected, next, true, false)
    }

    pub fn convert_snapshot_if_current(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.replace_snapshot_if_current_inner(expected, next, true, true)
    }

    pub(super) fn replace_snapshot_if_current_inner(
        &mut self,
        expected: EntitySnapshot,
        next: EntitySnapshot,
        journal_commit: bool,
        allow_type_change: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let entity_id = expected.id;
        let type_change_valid = if allow_type_change {
            expected.type_id != next.type_id
                && expected.type_name != next.type_name
                && expected.position == next.position
        } else {
            expected.type_id == next.type_id && expected.type_name == next.type_name
        };
        if expected.id != next.id || expected.uuid != next.uuid || !type_change_valid {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if expected.retained.item_pickup_claim.is_some()
            && (next.retained.item_pickup_claim != expected.retained.item_pickup_claim
                || next.item_stack != expected.item_stack
                || next.lifecycle != expected.lifecycle
                || next.type_id != expected.type_id
                || next.type_name != expected.type_name
                || next.position != expected.position)
        {
            return Ok(false);
        }
        let Some(source) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        let Some(target) = RegionKey::from_position(next.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(source)
            || self.in_flight_transfers.contains_key(&expected.id)
            || self.snapshot(expected.id)?.as_ref() != Some(&expected)
        {
            return Ok(false);
        }
        let changes_vehicle_indexes = source != target
            || snapshot_vehicle_reference(&expected) != snapshot_vehicle_reference(&next);
        let vehicle_indexes = if changes_vehicle_indexes {
            Some(self.vehicle_indexes_after_replacements(std::iter::once(&next))?)
        } else {
            None
        };
        self.ensure_region(target)?;
        let mutation_count = if source == target { 1 } else { 2 };
        let (first_sequence, sequence) = self.commit_state.reserve_sequences(mutation_count)?;
        let source_lease = self
            .ownership
            .lease(source)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let mut mutations = BTreeMap::new();
        if source == target {
            mutations.insert(
                source_lease.lane,
                vec![SequencedRegionMutation {
                    sequence,
                    lease: source_lease,
                    mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                        expected: Box::new(expected),
                        next: Box::new(next),
                        allow_type_change,
                    },
                }],
            );
        } else {
            let target_lease = self
                .ownership
                .lease(target)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations.insert(
                source_lease.lane,
                vec![SequencedRegionMutation {
                    sequence: first_sequence + 1,
                    lease: source_lease,
                    mutation: RegionOwnerMutation::RemoveIfCurrent(Box::new(expected)),
                }],
            );
            mutations.insert(
                target_lease.lane,
                vec![SequencedRegionMutation {
                    sequence,
                    lease: target_lease,
                    mutation: RegionOwnerMutation::InsertSnapshot(Box::new(next)),
                }],
            );
        }
        match self.execute_mutations_with_stats(mutations, sequence, journal_commit) {
            Ok(_) => {
                self.locations.insert(entity_id, target);
                if let Some((vehicle_passengers, passenger_vehicles)) = vehicle_indexes {
                    self.vehicle_passengers = vehicle_passengers;
                    self.passenger_vehicles = passenger_vehicles;
                }
                Ok(true)
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn merge_item_snapshots_if_current(
        &mut self,
        survivor_expected: EntitySnapshot,
        survivor_next: EntitySnapshot,
        consumed_expected: EntitySnapshot,
    ) -> Result<bool, RegionOwnerLaneError> {
        if survivor_expected.id == consumed_expected.id
            || survivor_expected.id != survivor_next.id
            || survivor_expected.uuid != survivor_next.uuid
            || survivor_expected.retained.item_pickup_claim.is_some()
            || consumed_expected.retained.item_pickup_claim.is_some()
            || survivor_expected.type_name != "minecraft:item"
            || survivor_next.type_name != "minecraft:item"
            || consumed_expected.type_name != "minecraft:item"
            || survivor_expected.position != survivor_next.position
            || survivor_expected.vehicle.is_some()
            || survivor_next.vehicle.is_some()
            || consumed_expected.vehicle.is_some()
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(survivor_key) = RegionKey::from_position(survivor_expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        let Some(consumed_key) = RegionKey::from_position(consumed_expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&survivor_expected.id).copied() != Some(survivor_key)
            || self.locations.get(&consumed_expected.id).copied() != Some(consumed_key)
            || self.in_flight_transfers.contains_key(&survivor_expected.id)
            || self.in_flight_transfers.contains_key(&consumed_expected.id)
            || self.snapshot(survivor_expected.id)?.as_ref() != Some(&survivor_expected)
            || self.snapshot(consumed_expected.id)?.as_ref() != Some(&consumed_expected)
        {
            return Ok(false);
        }
        if self.passenger_vehicles.contains_key(&survivor_expected.id)
            || self.passenger_vehicles.contains_key(&consumed_expected.id)
            || self.vehicle_passengers.contains_key(&survivor_expected.id)
            || self.vehicle_passengers.contains_key(&consumed_expected.id)
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(2)?;
        let survivor_lease = self
            .ownership
            .lease(survivor_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let consumed_lease = self
            .ownership
            .lease(consumed_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        mutations
            .entry(survivor_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: first_sequence + 1,
                lease: survivor_lease,
                mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                    expected: Box::new(survivor_expected),
                    next: Box::new(survivor_next),
                    allow_type_change: false,
                },
            });
        mutations
            .entry(consumed_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: sequence_watermark,
                lease: consumed_lease,
                mutation: RegionOwnerMutation::RemoveIfCurrent(Box::new(consumed_expected.clone())),
            });
        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => {
                self.locations.remove(&consumed_expected.id);
                self.uuids.remove(&consumed_expected.uuid);
                Ok(true)
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn commit_villager_inventory_pickup_if_current(
        &mut self,
        commit: VillagerInventoryPickupCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let VillagerInventoryPickupCommit {
            villager: (villager_expected, villager_next),
            item: (item_expected, item_next),
            item_max_stack_size,
        } = commit;
        let item_stack = item_expected
            .item_stack
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if villager_expected.type_name != "minecraft:villager"
            || villager_expected.lifecycle != EntityLifecycle::Alive
            || villager_next.lifecycle != EntityLifecycle::Alive
            || item_expected.type_name != "minecraft:item"
            || item_expected.lifecycle != EntityLifecycle::Alive
            || item_next
                .as_ref()
                .is_some_and(|next| next.lifecycle != EntityLifecycle::Alive)
            || villager_expected.id == item_expected.id
            || villager_expected.uuid == item_expected.uuid
            || villager_expected.id != villager_next.id
            || villager_expected.uuid != villager_next.uuid
            || villager_expected.type_id != villager_next.type_id
            || villager_expected.position != villager_next.position
            || villager_expected.vehicle.is_some()
            || villager_next.vehicle.is_some()
            || item_expected.vehicle.is_some()
            || item_next
                .as_ref()
                .is_some_and(|next| next.vehicle.is_some())
            || villager_expected.retained.item_pickup_claim.is_some()
            || villager_next.retained.item_pickup_claim.is_some()
            || item_expected.retained.item_pickup_claim.is_some()
            || item_next
                .as_ref()
                .is_some_and(|next| next.retained.item_pickup_claim.is_some())
            || item_max_stack_size <= 0
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let villager_min_x = villager_expected.position.x - 1.3;
        let villager_max_x = villager_expected.position.x + 1.3;
        let villager_min_y = villager_expected.position.y;
        let villager_max_y = villager_expected.position.y + 1.95;
        let villager_min_z = villager_expected.position.z - 1.3;
        let villager_max_z = villager_expected.position.z + 1.3;
        let item_min_x = item_expected.position.x - 0.125;
        let item_max_x = item_expected.position.x + 0.125;
        let item_min_y = item_expected.position.y;
        let item_max_y = item_expected.position.y + 0.25;
        let item_min_z = item_expected.position.z - 0.125;
        let item_max_z = item_expected.position.z + 0.125;
        if !villager_expected.position.is_finite() || !item_expected.position.is_finite() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if item_max_x < villager_min_x
            || item_min_x > villager_max_x
            || item_max_y < villager_min_y
            || item_min_y > villager_max_y
            || item_max_z < villager_min_z
            || item_min_z > villager_max_z
        {
            let dx = villager_expected.position.x - item_expected.position.x;
            let dy = villager_expected.position.y - item_expected.position.y;
            let dz = villager_expected.position.z - item_expected.position.z;
            let targeted_share_in_range = item_expected.retained.villager_food_recipient
                == Some(villager_expected.id)
                && dx.mul_add(dx, dy.mul_add(dy, dz * dz))
                    <= crate::villager_population_26_1_2::VILLAGER_SHARED_FOOD_PICKUP_RADIUS
                        .powi(2);
            if !targeted_share_in_range {
                return Ok(false);
            }
        }

        let mut derived_population = villager_expected
            .retained
            .villager_population
            .clone()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let remainder = derived_population
            .add_to_inventory(item_stack.clone(), item_max_stack_size)
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        let mut derived_villager_next = villager_expected.clone();
        derived_villager_next.retained.villager_population = Some(derived_population);
        let derived_item_next = remainder.map(|remainder| {
            let mut next = item_expected.clone();
            next.item_stack = Some(remainder);
            next
        });
        if villager_next != derived_villager_next || item_next != derived_item_next {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let villager_key = RegionKey::from_position(villager_expected.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let item_key = RegionKey::from_position(item_expected.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if self.locations.get(&villager_expected.id).copied() != Some(villager_key)
            || self.locations.get(&item_expected.id).copied() != Some(item_key)
            || self.in_flight_transfers.contains_key(&villager_expected.id)
            || self.in_flight_transfers.contains_key(&item_expected.id)
            || self.snapshot(villager_expected.id)?.as_ref() != Some(&villager_expected)
            || self.snapshot(item_expected.id)?.as_ref() != Some(&item_expected)
        {
            return Ok(false);
        }
        if [villager_expected.id, item_expected.id].iter().any(|id| {
            self.passenger_vehicles.contains_key(id) || self.vehicle_passengers.contains_key(id)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(2)?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        let villager_lease = self
            .ownership
            .lease(villager_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        mutations
            .entry(villager_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: first_sequence + 1,
                lease: villager_lease,
                mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                    expected: Box::new(villager_expected),
                    next: Box::new(villager_next),
                    allow_type_change: false,
                },
            });
        let item_lease = self
            .ownership
            .lease(item_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let item_was_removed = item_next.is_none();
        let (item_id, item_uuid) = (item_expected.id, item_expected.uuid);
        let item_mutation = match item_next {
            Some(next) => RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                expected: Box::new(item_expected),
                next: Box::new(next),
                allow_type_change: false,
            },
            None => RegionOwnerMutation::RemoveIfCurrent(Box::new(item_expected)),
        };
        mutations
            .entry(item_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: sequence_watermark,
                lease: item_lease,
                mutation: item_mutation,
            });

        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => {
                if item_was_removed {
                    self.locations.remove(&item_id);
                    self.uuids.remove(&item_uuid);
                }
                Ok(true)
            }
            Err(
                RegionOwnerLaneError::InvalidMutation
                | RegionOwnerLaneError::StaleLease
                | RegionOwnerLaneError::UnknownEntity,
            ) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn commit_villager_food_share_if_current(
        &mut self,
        commit: VillagerFoodShareCommit,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let VillagerFoodShareCommit {
            donor: (donor_expected, donor_next),
            recipient,
            thrown_item,
            food_items,
            item_max_stack_size,
            current_tick,
        } = commit;
        if donor_expected.id == recipient.id
            || donor_expected.uuid == recipient.uuid
            || donor_expected.type_name != "minecraft:villager"
            || recipient.type_name != "minecraft:villager"
            || donor_expected.lifecycle != EntityLifecycle::Alive
            || donor_next.lifecycle != EntityLifecycle::Alive
            || recipient.lifecycle != EntityLifecycle::Alive
            || donor_expected.id != donor_next.id
            || donor_expected.uuid != donor_next.uuid
            || donor_expected.type_id != donor_next.type_id
            || donor_expected.position != donor_next.position
            || donor_expected.vehicle.is_some()
            || donor_next.vehicle.is_some()
            || recipient.vehicle.is_some()
            || donor_expected.retained.item_pickup_claim.is_some()
            || donor_next.retained.item_pickup_claim.is_some()
            || recipient.retained.item_pickup_claim.is_some()
            || item_max_stack_size <= 0
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let dx = recipient.position.x - donor_expected.position.x;
        let dy = recipient.position.y - donor_expected.position.y;
        let dz = recipient.position.z - donor_expected.position.z;
        let distance_squared = dx.mul_add(dx, dy.mul_add(dy, dz * dz));
        if !distance_squared.is_finite()
            || distance_squared
                > crate::villager_population_26_1_2::VILLAGER_COURTSHIP_DISTANCE_SQUARED
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let recipient_population = recipient
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if !recipient_population.wants_more_food(food_items) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let mut derived_population = donor_expected
            .retained
            .villager_population
            .clone()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let shared = derived_population
            .inventory
            .extract_food_share(food_items, item_max_stack_size)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let mut derived_donor_next = donor_expected.clone();
        derived_donor_next.retained.villager_population = Some(derived_population);
        if donor_next != derived_donor_next {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let thrown_uuid = thrown_item
            .uuid
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if self.uuids.contains_key(&thrown_uuid) {
            return Ok(None);
        }
        let throw_length = distance_squared.sqrt();
        let expected_velocity = if throw_length > 0.0 {
            Vec3::new(
                dx / throw_length * crate::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
                dy / throw_length * crate::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
                dz / throw_length * crate::villager_population_26_1_2::VILLAGER_ITEM_THROW_SPEED,
            )
        } else {
            Vec3::new(0.0, 0.0, 0.0)
        };
        let expected_position = Vec3::new(
            donor_expected.position.x,
            donor_expected.position.y
                + crate::villager_population_26_1_2::VILLAGER_ITEM_THROW_Y_OFFSET,
            donor_expected.position.z,
        );
        if thrown_item.type_name != "minecraft:item"
            || thrown_item.position != expected_position
            || thrown_item.velocity != expected_velocity
            || thrown_item.item_stack.as_ref() != Some(&shared)
            || thrown_item.vehicle.is_some()
            || thrown_item.retained.spawn_tick != current_tick
            || thrown_item.retained.item_pickup_ready_tick
                != current_tick.checked_add(
                    crate::villager_population_26_1_2::VILLAGER_ITEM_THROW_PICKUP_DELAY_TICKS,
                )
            || thrown_item.retained.item_pickup_owner_block.is_some()
            || thrown_item.retained.villager_food_recipient != Some(recipient.id)
            || !thrown_item.position.is_finite()
            || !thrown_item.velocity.is_finite()
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let donor_key = RegionKey::from_position(donor_expected.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let recipient_key = RegionKey::from_position(recipient.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let thrown_key = RegionKey::from_position(thrown_item.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        if self.locations.get(&donor_expected.id).copied() != Some(donor_key)
            || self.locations.get(&recipient.id).copied() != Some(recipient_key)
            || self.in_flight_transfers.contains_key(&donor_expected.id)
            || self.in_flight_transfers.contains_key(&recipient.id)
            || self.snapshot(donor_expected.id)?.as_ref() != Some(&donor_expected)
            || self.snapshot(recipient.id)?.as_ref() != Some(&recipient)
            || [donor_expected.id, recipient.id].iter().any(|id| {
                self.passenger_vehicles.contains_key(id) || self.vehicle_passengers.contains_key(id)
            })
        {
            return Ok(None);
        }
        self.ensure_region(thrown_key)?;
        let mut child_id_cursor = self.next_id;
        let thrown_id = loop {
            child_id_cursor = child_id_cursor
                .max(0)
                .checked_add(1)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            let candidate = EntityId(child_id_cursor);
            if !self.locations.contains_key(&candidate) {
                break candidate;
            }
        };
        let thrown_snapshot = snapshot_from_spawn(thrown_id, thrown_uuid, thrown_item);
        if self.locations.contains_key(&thrown_snapshot.id)
            || self.uuids.contains_key(&thrown_snapshot.uuid)
        {
            return Ok(None);
        }

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(2)?;
        let donor_lease = self
            .ownership
            .lease(donor_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let thrown_lease = self
            .ownership
            .lease(thrown_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        mutations
            .entry(donor_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: first_sequence + 1,
                lease: donor_lease,
                mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                    expected: Box::new(donor_expected),
                    next: Box::new(donor_next),
                    allow_type_change: false,
                },
            });
        mutations
            .entry(thrown_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence: sequence_watermark,
                lease: thrown_lease,
                mutation: RegionOwnerMutation::InsertSnapshot(Box::new(thrown_snapshot.clone())),
            });
        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => {
                self.next_id = thrown_snapshot.id.0;
                self.locations.insert(thrown_snapshot.id, thrown_key);
                self.uuids.insert(thrown_snapshot.uuid, thrown_snapshot.id);
                Ok(Some(thrown_snapshot))
            }
            Err(
                RegionOwnerLaneError::InvalidMutation
                | RegionOwnerLaneError::StaleLease
                | RegionOwnerLaneError::UnknownEntity,
            ) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn commit_villager_courtship_if_current(
        &mut self,
        commit: VillagerCourtshipCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let VillagerCourtshipCommit {
            parents,
            current_tick,
            food_items,
            deterministic_seed,
        } = commit;
        let dx = parents[0].0.position.x - parents[1].0.position.x;
        let dy = parents[0].0.position.y - parents[1].0.position.y;
        let dz = parents[0].0.position.z - parents[1].0.position.z;
        let distance_squared = dx.mul_add(dx, dy.mul_add(dy, dz * dz));
        if parents[0].0.id == parents[1].0.id
            || parents[0].0.uuid == parents[1].0.uuid
            || !distance_squared.is_finite()
            || distance_squared > 64.0
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let mut expected_ids = HashSet::with_capacity(2);
        let mut expected_uuids = HashSet::with_capacity(2);
        let mut parent_locations = Vec::with_capacity(2);
        for (index, (expected, next)) in parents.iter().enumerate() {
            if expected.type_name != "minecraft:villager"
                || expected.lifecycle != EntityLifecycle::Alive
                || next.lifecycle != EntityLifecycle::Alive
                || expected.id != next.id
                || expected.uuid != next.uuid
                || expected.type_id != next.type_id
                || expected.position != next.position
                || expected.vehicle.is_some()
                || next.vehicle.is_some()
                || expected.retained.item_pickup_claim.is_some()
                || next.retained.item_pickup_claim.is_some()
                || !expected_ids.insert(expected.id)
                || !expected_uuids.insert(expected.uuid)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let mut derived_population = expected
                .retained
                .villager_population
                .clone()
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            derived_population
                .start_pending_birth(
                    parents[1 - index].0.uuid,
                    current_tick,
                    deterministic_seed,
                    false,
                    food_items,
                )
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
            let mut derived_next = expected.clone();
            derived_next.retained.villager_population = Some(derived_population);
            derived_next.goal = GoalState::FollowTarget {
                target: parents[1 - index].0.id,
                speed: crate::villager_population_26_1_2::VILLAGER_COURTSHIP_SPEED,
            };
            if *next != derived_next {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let Some(key) = RegionKey::from_position(expected.position) else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            if self.locations.get(&expected.id).copied() != Some(key)
                || self.in_flight_transfers.contains_key(&expected.id)
                || self.snapshot(expected.id)?.as_ref() != Some(expected)
            {
                return Ok(false);
            }
            parent_locations.push(key);
        }
        if expected_ids.iter().any(|id| {
            self.passenger_vehicles.contains_key(id) || self.vehicle_passengers.contains_key(id)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(2)?;
        let mut sequence = first_sequence;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for ((expected, next), key) in parents.into_iter().zip(parent_locations) {
            sequence += 1;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence,
                    lease,
                    mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                        expected: Box::new(expected),
                        next: Box::new(next),
                        allow_type_change: false,
                    },
                });
        }
        debug_assert_eq!(sequence, sequence_watermark);

        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => Ok(true),
            Err(
                RegionOwnerLaneError::InvalidMutation
                | RegionOwnerLaneError::StaleLease
                | RegionOwnerLaneError::UnknownEntity,
            ) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn commit_villager_no_bed_if_current(
        &mut self,
        commit: VillagerNoBedCommit,
    ) -> Result<bool, RegionOwnerLaneError> {
        let VillagerNoBedCommit {
            parents,
            current_tick,
            food_items,
        } = commit;
        let first_population = parents[0]
            .0
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let second_population = parents[1]
            .0
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let first_pending = first_population
            .pending_birth
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let second_pending = second_population
            .pending_birth
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let dx = parents[0].0.position.x - parents[1].0.position.x;
        let dy = parents[0].0.position.y - parents[1].0.position.y;
        let dz = parents[0].0.position.z - parents[1].0.position.z;
        let distance_squared = dx.mul_add(dx, dy.mul_add(dy, dz * dz));
        if parents[0].0.id == parents[1].0.id
            || parents[0].0.uuid == parents[1].0.uuid
            || !distance_squared.is_finite()
            || distance_squared
                > crate::villager_population_26_1_2::VILLAGER_COURTSHIP_DISTANCE_SQUARED
            || first_pending.partner_uuid != parents[1].0.uuid
            || second_pending.partner_uuid != parents[0].0.uuid
            || first_pending.started_tick != second_pending.started_tick
            || first_pending.ready_tick != second_pending.ready_tick
            || current_tick < first_pending.ready_tick
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let mut expected_ids = HashSet::with_capacity(2);
        let mut expected_uuids = HashSet::with_capacity(2);
        let mut parent_locations = Vec::with_capacity(2);
        for (expected, next) in &parents {
            if expected.type_name != "minecraft:villager"
                || expected.lifecycle != EntityLifecycle::Alive
                || next.lifecycle != EntityLifecycle::Alive
                || expected.id != next.id
                || expected.uuid != next.uuid
                || expected.type_id != next.type_id
                || expected.position != next.position
                || expected.vehicle.is_some()
                || next.vehicle.is_some()
                || expected.retained.item_pickup_claim.is_some()
                || next.retained.item_pickup_claim.is_some()
                || !expected_ids.insert(expected.id)
                || !expected_uuids.insert(expected.uuid)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let mut derived_population = expected
                .retained
                .villager_population
                .clone()
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            derived_population
                .finish_courtship_without_child(current_tick, food_items)
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
            let mut derived_next = expected.clone();
            derived_next.retained.villager_population = Some(derived_population);
            derived_next.goal = GoalState::Idle;
            if *next != derived_next {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let key = RegionKey::from_position(expected.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if self.locations.get(&expected.id).copied() != Some(key)
                || self.in_flight_transfers.contains_key(&expected.id)
                || self.snapshot(expected.id)?.as_ref() != Some(expected)
            {
                return Ok(false);
            }
            parent_locations.push(key);
        }
        if expected_ids.iter().any(|id| {
            self.passenger_vehicles.contains_key(id) || self.vehicle_passengers.contains_key(id)
        }) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(2)?;
        let mut sequence = first_sequence;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for ((expected, next), key) in parents.into_iter().zip(parent_locations) {
            sequence += 1;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence,
                    lease,
                    mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                        expected: Box::new(expected),
                        next: Box::new(next),
                        allow_type_change: false,
                    },
                });
        }
        debug_assert_eq!(sequence, sequence_watermark);
        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => Ok(true),
            Err(
                RegionOwnerLaneError::InvalidMutation
                | RegionOwnerLaneError::StaleLease
                | RegionOwnerLaneError::UnknownEntity,
            ) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn commit_villager_birth_if_current(
        &mut self,
        commit: VillagerBirthCommit,
    ) -> Result<Option<EntitySnapshot>, RegionOwnerLaneError> {
        let VillagerBirthCommit {
            parents,
            child,
            current_tick,
            food_items,
        } = commit;
        let child_uuid = child.uuid.ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let mut child_id_cursor = self.next_id;
        let child_id = loop {
            child_id_cursor = child_id_cursor
                .max(0)
                .checked_add(1)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            let candidate = EntityId(child_id_cursor);
            if !self.locations.contains_key(&candidate) {
                break candidate;
            }
        };
        let child = snapshot_from_spawn(child_id, child_uuid, child);
        let first_population = parents[0]
            .0
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let second_population = parents[1]
            .0
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let first_pending = first_population
            .pending_birth
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let second_pending = second_population
            .pending_birth
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let child_population = child
            .retained
            .villager_population
            .as_ref()
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let claimed_home = child_population
            .claimed_home
            .as_deref()
            .filter(|home| !home.is_empty())
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let expected_child_uuid =
            crate::villager_population_26_1_2::deterministic_villager_child_uuid(
                parents[0].0.uuid,
                parents[1].0.uuid,
                claimed_home,
                first_pending.started_tick,
            );
        let dx = parents[0].0.position.x - parents[1].0.position.x;
        let dy = parents[0].0.position.y - parents[1].0.position.y;
        let dz = parents[0].0.position.z - parents[1].0.position.z;
        let distance_squared = dx.mul_add(dx, dy.mul_add(dy, dz * dz));
        if parents[0].0.id == parents[1].0.id
            || parents[0].0.uuid == parents[1].0.uuid
            || !distance_squared.is_finite()
            || distance_squared > 5.0
            || first_pending.partner_uuid != parents[1].0.uuid
            || second_pending.partner_uuid != parents[0].0.uuid
            || first_pending.started_tick != second_pending.started_tick
            || first_pending.ready_tick != second_pending.ready_tick
            || current_tick < first_pending.ready_tick
            || child_uuid != expected_child_uuid
            || child.type_name != "minecraft:villager"
            || child.vehicle.is_some()
            || child.retained.item_pickup_claim.is_some()
            || child_population.age_ticks
                != crate::villager_population_26_1_2::VILLAGER_BABY_START_AGE_TICKS
            || child_population.food_level != 0
            || child_population
                .inventory
                .slots()
                .iter()
                .any(Option::is_some)
            || child_population.pending_birth.is_some()
            || !child.rotation.is_finite()
            || !child.velocity.is_finite()
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }

        let mut expected_ids = HashSet::with_capacity(2);
        let mut expected_uuids = HashSet::with_capacity(2);
        let mut parent_locations = Vec::with_capacity(2);
        for (expected, next) in &parents {
            if expected.type_name != "minecraft:villager"
                || expected.lifecycle != EntityLifecycle::Alive
                || next.lifecycle != EntityLifecycle::Alive
                || expected.id != next.id
                || expected.uuid != next.uuid
                || expected.type_id != next.type_id
                || expected.position != next.position
                || expected.vehicle.is_some()
                || next.vehicle.is_some()
                || expected.retained.item_pickup_claim.is_some()
                || next.retained.item_pickup_claim.is_some()
                || !expected_ids.insert(expected.id)
                || !expected_uuids.insert(expected.uuid)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let mut derived_population = expected
                .retained
                .villager_population
                .clone()
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            derived_population
                .finish_successful_birth(current_tick, food_items)
                .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
            let mut derived_next = expected.clone();
            derived_next.retained.villager_population = Some(derived_population);
            derived_next.goal = GoalState::Idle;
            if *next != derived_next {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            let Some(key) = RegionKey::from_position(expected.position) else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            if self.locations.get(&expected.id).copied() != Some(key)
                || self.in_flight_transfers.contains_key(&expected.id)
                || self.snapshot(expected.id)?.as_ref() != Some(expected)
            {
                return Ok(None);
            }
            parent_locations.push(key);
        }

        let Some(child_key) = RegionKey::from_position(child.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if expected_ids.contains(&child.id)
            || expected_uuids.contains(&child.uuid)
            || self.locations.contains_key(&child.id)
            || self.uuids.contains_key(&child.uuid)
            || self.passenger_vehicles.contains_key(&child.id)
            || self.vehicle_passengers.contains_key(&child.id)
            || expected_ids.iter().any(|id| {
                self.passenger_vehicles.contains_key(id) || self.vehicle_passengers.contains_key(id)
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        self.ensure_region(child_key)?;

        let (first_sequence, sequence_watermark) = self.commit_state.reserve_sequences(3)?;
        let mut sequence = first_sequence;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for ((expected, next), key) in parents.into_iter().zip(parent_locations) {
            sequence += 1;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence,
                    lease,
                    mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                        expected: Box::new(expected),
                        next: Box::new(next),
                        allow_type_change: false,
                    },
                });
        }
        sequence += 1;
        let child_lease = self
            .ownership
            .lease(child_key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        mutations
            .entry(child_lease.lane)
            .or_default()
            .push(SequencedRegionMutation {
                sequence,
                lease: child_lease,
                mutation: RegionOwnerMutation::InsertSnapshot(Box::new(child.clone())),
            });
        debug_assert_eq!(sequence, sequence_watermark);

        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => {
                self.next_id = child.id.0;
                self.locations.insert(child.id, child_key);
                self.uuids.insert(child.uuid, child.id);
                Ok(Some(child))
            }
            Err(
                RegionOwnerLaneError::InvalidMutation
                | RegionOwnerLaneError::StaleLease
                | RegionOwnerLaneError::UnknownEntity,
            ) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn replace_snapshots_if_current(
        &mut self,
        snapshots: impl IntoIterator<Item = (EntitySnapshot, EntitySnapshot)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        if snapshots.is_empty() {
            return Ok(true);
        }

        let mut entity_ids = HashSet::with_capacity(snapshots.len());
        let mut locations = Vec::with_capacity(snapshots.len());
        let mut mutation_count = 0_usize;
        for (expected, next) in &snapshots {
            if expected.id != next.id
                || expected.uuid != next.uuid
                || !entity_ids.insert(expected.id)
                || !next.rotation.is_finite()
                || !next.velocity.is_finite()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if expected.retained.item_pickup_claim.is_some()
                && (next.retained.item_pickup_claim != expected.retained.item_pickup_claim
                    || next.item_stack != expected.item_stack
                    || next.lifecycle != expected.lifecycle
                    || next.type_id != expected.type_id
                    || next.type_name != expected.type_name
                    || next.position != expected.position)
            {
                return Ok(false);
            }
            let Some(source) = RegionKey::from_position(expected.position) else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            let Some(target) = RegionKey::from_position(next.position) else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            if self.locations.get(&expected.id).copied() != Some(source)
                || self.in_flight_transfers.contains_key(&expected.id)
            {
                return Ok(false);
            }
            mutation_count = mutation_count
                .checked_add(if source == target { 1 } else { 2 })
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            locations.push((expected.id, source, target));
        }
        self.commit_state.ensure_committed_state()?;
        let changes_vehicle_indexes = locations.iter().any(|(_, source, target)| source != target)
            || snapshots.iter().any(|(expected, next)| {
                snapshot_vehicle_reference(expected) != snapshot_vehicle_reference(next)
            });
        // In-place state changes are validated atomically by owner preparation.
        // Topology changes keep their coordinator preflight before index planning.
        if changes_vehicle_indexes {
            let current = self.snapshots_for_ids(&entity_ids)?;
            if current.len() != snapshots.len()
                || snapshots.iter().any(|(expected, _)| {
                    current
                        .binary_search_by_key(&expected.id, |snapshot| snapshot.id)
                        .ok()
                        .is_none_or(|index| current[index] != *expected)
                })
            {
                return Ok(false);
            }
        }
        for (_, _, target) in &locations {
            self.ensure_region(*target)?;
        }
        // Timer and other in-region state updates cannot change passenger
        // topology. Do not snapshot the whole population for those batches.
        let vehicle_indexes = if changes_vehicle_indexes {
            Some(self.vehicle_indexes_after_replacements(snapshots.iter().map(|(_, next)| next))?)
        } else {
            None
        };

        let (first_sequence, sequence_watermark) =
            self.commit_state.reserve_sequences(mutation_count)?;
        let mut sequence = first_sequence;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for ((expected, next), (_, source, target)) in
            snapshots.into_iter().zip(locations.iter().copied())
        {
            let source_lease = self
                .ownership
                .lease(source)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            if source == target {
                sequence += 1;
                mutations
                    .entry(source_lease.lane)
                    .or_default()
                    .push(SequencedRegionMutation {
                        sequence,
                        lease: source_lease,
                        mutation: RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                            expected: Box::new(expected),
                            next: Box::new(next),
                            allow_type_change: false,
                        },
                    });
                continue;
            }

            let target_lease = self
                .ownership
                .lease(target)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            sequence += 1;
            mutations
                .entry(source_lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence,
                    lease: source_lease,
                    mutation: RegionOwnerMutation::RemoveIfCurrent(Box::new(expected)),
                });
            sequence += 1;
            mutations
                .entry(target_lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence,
                    lease: target_lease,
                    mutation: RegionOwnerMutation::InsertSnapshot(Box::new(next)),
                });
        }
        debug_assert_eq!(sequence, sequence_watermark);
        match self.execute_mutations(mutations, sequence_watermark) {
            Ok(()) => {
                for (entity, _, target) in locations {
                    self.locations.insert(entity, target);
                }
                if let Some((vehicle_passengers, passenger_vehicles)) = vehicle_indexes {
                    self.vehicle_passengers = vehicle_passengers;
                    self.passenger_vehicles = passenger_vehicles;
                }
                Ok(true)
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    fn ensure_region(&mut self, key: RegionKey) -> Result<RegionLease, RegionOwnerLaneError> {
        if let Some(lease) = self.ownership.lease(key) {
            return Ok(lease);
        }
        let mut region_counts = self
            .lanes
            .keys()
            .map(|lane| (*lane, 0usize))
            .collect::<BTreeMap<_, _>>();
        for lease in self.ownership.leases() {
            let count = region_counts
                .get_mut(&lease.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            *count += 1;
        }
        let lane = region_counts
            .into_iter()
            .min_by_key(|(lane, count)| (*count, *lane))
            .map(|(lane, _)| lane)
            .ok_or(RegionOwnerLaneError::InvalidLaneCount)?;
        let lease = self
            .ownership
            .assign(key, lane)
            .map_err(|_| RegionOwnerLaneError::Busy)?;
        let installed = self
            .lanes
            .get(&lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?
            .install_region(lease, EntityStore::new());
        if let Err(failure) = installed {
            if failure.recovered.is_some() {
                self.ownership
                    .unassign(lease)
                    .map_err(|_| RegionOwnerLaneError::Busy)?;
            } else {
                self.commit_state.mark_outcome_unknown();
            }
            return Err(failure.error);
        }
        Ok(lease)
    }

    fn next_owner_id(&self) -> Result<EntityId, RegionOwnerLaneError> {
        let mut next = self.next_id.max(0);
        loop {
            next = next
                .checked_add(1)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            let id = EntityId(next);
            if !self.locations.contains_key(&id) {
                return Ok(id);
            }
        }
    }

    fn vehicle_component_ids(
        &self,
        entities: &HashSet<EntityId>,
    ) -> Result<HashSet<EntityId>, RegionOwnerLaneError> {
        let mut component = HashSet::new();
        for &entity in entities {
            let mut leader = entity;
            let mut steps = 0usize;
            while let Some(&vehicle) = self.passenger_vehicles.get(&leader) {
                steps += 1;
                if steps > self.locations.len() {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                leader = vehicle;
            }
            steps = 0;
            loop {
                steps += 1;
                if steps > self.locations.len().saturating_add(1) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                component.insert(leader);
                let Some(&passenger) = self.vehicle_passengers.get(&leader) else {
                    break;
                };
                leader = passenger;
            }
        }
        Ok(component)
    }

    fn index_vehicle_link(&mut self, snapshot: &EntitySnapshot) {
        let Some(passenger) = snapshot_vehicle_reference(snapshot) else {
            return;
        };
        debug_assert!(
            self.vehicle_passengers
                .insert(snapshot.id, passenger)
                .is_none()
        );
        debug_assert!(
            self.passenger_vehicles
                .insert(passenger, snapshot.id)
                .is_none()
        );
    }

    fn vehicle_indexes_after_replacements<'a>(
        &self,
        replacements: impl IntoIterator<Item = &'a EntitySnapshot>,
    ) -> Result<VehicleIndexes, RegionOwnerLaneError> {
        let mut snapshots = self
            .snapshots()?
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<BTreeMap<_, _>>();
        for replacement in replacements {
            if !snapshots.contains_key(&replacement.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            snapshots.insert(replacement.id, replacement.clone());
        }

        let mut vehicle_passengers = HashMap::new();
        let mut passenger_vehicles = HashMap::new();
        for snapshot in snapshots.values() {
            let Some(passenger) = snapshot_vehicle_reference(snapshot) else {
                continue;
            };
            let Some(passenger_snapshot) = snapshots.get(&passenger) else {
                return Err(RegionOwnerLaneError::InvalidMutation);
            };
            if passenger == snapshot.id
                || RegionKey::from_position(passenger_snapshot.position)
                    != RegionKey::from_position(snapshot.position)
                || vehicle_passengers.insert(snapshot.id, passenger).is_some()
                || passenger_vehicles.insert(passenger, snapshot.id).is_some()
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        for &start in snapshots.keys() {
            let mut current = start;
            let mut visited = HashSet::new();
            while let Some(&passenger) = vehicle_passengers.get(&current) {
                if !visited.insert(current) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                current = passenger;
            }
        }
        Ok((vehicle_passengers, passenger_vehicles))
    }

    fn remove_vehicle_links(&mut self, snapshot: &EntitySnapshot) {
        if let Some(passenger) = self.vehicle_passengers.remove(&snapshot.id)
            && self.passenger_vehicles.get(&passenger).copied() == Some(snapshot.id)
        {
            self.passenger_vehicles.remove(&passenger);
        }
        if let Some(vehicle) = self.passenger_vehicles.remove(&snapshot.id)
            && self.vehicle_passengers.get(&vehicle).copied() == Some(snapshot.id)
        {
            self.vehicle_passengers.remove(&vehicle);
        }
    }

    pub fn set_animal_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.set_animal_states_if_current_inner(states, true)
    }

    pub(super) fn set_animal_states_if_current_inner(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        let mut ids = HashSet::with_capacity(states.len());
        for (expected, _) in &states {
            if expected.animal.is_none() || !ids.insert(expected.id) {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if self.locations.get(&expected.id).copied()
                != RegionKey::from_position(expected.position)
            {
                return Ok(false);
            }
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(states.len())?;
        let mut mutations = BTreeMap::new();
        for (offset, (expected, animal)) in states.into_iter().enumerate() {
            let key = self.locations[&expected.id];
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_insert_with(Vec::new)
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetAnimalStateIfCurrent {
                        expected: Box::new(expected),
                        animal,
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn set_goal(
        &mut self,
        entity: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionOwnerLaneError> {
        if goal_reference(&goal).is_some_and(|target| !self.locations.contains_key(&target)) {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        let key = self.locations[&entity];
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::SetGoalIfCurrent {
                    expected: Box::new(expected),
                    goal,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn set_goals(
        &mut self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
    ) -> Result<usize, RegionOwnerLaneError> {
        self.set_goals_inner(goals, true)
    }

    pub(super) fn set_goals_inner(
        &mut self,
        goals: impl IntoIterator<Item = (EntityId, GoalState)>,
        journal_commit: bool,
    ) -> Result<usize, RegionOwnerLaneError> {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return Ok(0);
        }
        let mut ids = HashSet::with_capacity(goals.len());
        for (entity, goal) in &goals {
            if !ids.insert(*entity)
                || goal_reference(goal).is_some_and(|target| !self.locations.contains_key(&target))
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
        }
        let current = self
            .snapshots_for_ids(&ids)?
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<HashMap<_, _>>();
        if goals
            .iter()
            .any(|(entity, _)| !current.contains_key(entity))
        {
            return Ok(0);
        }
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(goals.len())?;
        let mut mutations = BTreeMap::new();
        for (offset, (entity, goal)) in goals.into_iter().enumerate() {
            let key = self.locations[&entity];
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            mutations
                .entry(lease.lane)
                .or_insert_with(Vec::new)
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetGoalIfCurrent {
                        expected: Box::new(current[&entity].clone()),
                        goal,
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => Ok(ids.len()),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(0),
            Err(error) => Err(error),
        }
    }

    pub fn set_item_stack(
        &mut self,
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        self.set_item_stack_if_current(expected, item_stack)
    }

    pub fn set_item_stack_if_current(
        &mut self,
        expected: EntitySnapshot,
        item_stack: Option<EntityItemStack>,
    ) -> Result<bool, RegionOwnerLaneError> {
        if expected.retained.item_pickup_claim.is_some() {
            return Ok(false);
        }
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(false);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::SetItemStackIfCurrent {
                    expected: Box::new(expected),
                    item_stack,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => Ok(true),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn resolve_item_pickup_claim(
        &mut self,
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
    ) -> Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError> {
        self.resolve_item_pickup_claim_inner(entity, claim, item_stack, true)
    }

    pub(super) fn resolve_item_pickup_claim_inner(
        &mut self,
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
        journal_commit: bool,
    ) -> Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError> {
        let Some(current) = self.snapshot(entity)? else {
            return Ok(None);
        };
        if current.retained.item_pickup_claim != Some(claim) {
            return Ok(None);
        }
        let Some(key) = RegionKey::from_position(current.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&entity).copied() != Some(key)
            || self.in_flight_transfers.contains_key(&entity)
        {
            return Ok(None);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let (mutation, resolution) = if let Some(item_stack) = item_stack {
            let mut next = current.clone();
            next.item_stack = Some(item_stack);
            next.retained.item_pickup_claim = None;
            (
                RegionOwnerMutation::ReplaceSnapshotIfCurrent {
                    expected: Box::new(current),
                    next: Box::new(next.clone()),
                    allow_type_change: false,
                },
                ItemPickupClaimResolution::Updated(next),
            )
        } else {
            (
                RegionOwnerMutation::RemoveIfCurrent(Box::new(current.clone())),
                ItemPickupClaimResolution::Removed(current),
            )
        };
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation,
            }],
        )]);
        match self.execute_mutations_with_stats(mutations, sequence, journal_commit) {
            Ok(_) => {
                if let ItemPickupClaimResolution::Removed(removed) = &resolution {
                    self.remove_vehicle_links(removed);
                    self.locations.remove(&removed.id);
                    self.uuids.remove(&removed.uuid);
                }
                Ok(Some(resolution))
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn set_position(
        &mut self,
        entity: EntityId,
        position: Vec3,
    ) -> Result<bool, RegionOwnerLaneError> {
        if !position.is_finite() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(false);
        };
        let state = EntityKinematics {
            id: entity,
            position,
            rotation: expected.rotation,
            velocity: expected.velocity,
            on_ground: expected.on_ground,
        };
        self.apply_kinematics_if_current([(expected, state)])
    }

    pub fn damage(
        &mut self,
        entity: EntityId,
        request: EntityDamageRequest,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        let Some(expected) = self.snapshot(entity)? else {
            return Ok(None);
        };
        self.damage_if_current(expected, request)
    }

    pub fn apply_kinematics_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
    ) -> Result<bool, RegionOwnerLaneError> {
        self.apply_kinematics_if_current_inner(states, true)
    }

    #[cfg(test)]
    pub(super) fn apply_kinematics_if_versioned_inner(
        &mut self,
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
        journal_commit: bool,
    ) -> Result<Option<Vec<EntityKinematics>>, RegionOwnerLaneError> {
        self.apply_kinematics_if_versioned_with_fence(expected, states, journal_commit)
            .map(|committed| committed.map(VersionedKinematicsCommit::into_states))
    }

    pub(super) fn apply_kinematics_if_versioned_with_fence(
        &mut self,
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
        journal_commit: bool,
    ) -> Result<Option<VersionedKinematicsCommit>, RegionOwnerLaneError> {
        if expected.authority != self.authority
            || expected.expected.is_empty()
            || expected.expected.len() != states.len()
            || expected.routes.len() != expected.expected.len()
        {
            #[cfg(feature = "load-bench")]
            eprintln!(
                "PHYSICS_VERSIONED_REJECT reason=shape expected={} states={}",
                expected.expected.len(),
                states.len()
            );
            return Ok(None);
        }
        let local_lane_apply = !journal_commit
            && expected
                .expected
                .iter()
                .zip(&expected.routes)
                .zip(&states)
                .all(|((previous, lease), state)| {
                    previous.eligible()
                        && previous.id == state.id
                        && state.is_finite()
                        && self.locations.get(&state.id).copied() == Some(lease.key)
                        && self.ownership.lease(lease.key) == Some(*lease)
                        && RegionKey::from_position(previous.expected.position) == Some(lease.key)
                        && RegionKey::from_position(state.position) == Some(lease.key)
                        && !self.vehicle_passengers.contains_key(&state.id)
                        && !self.passenger_vehicles.contains_key(&state.id)
                });
        if local_lane_apply {
            return self.apply_local_kinematics_if_versioned(expected, states);
        }
        #[cfg(feature = "load-bench")]
        let validation_started = std::time::Instant::now();
        let mut ids = HashSet::with_capacity(states.len());
        let mut by_region = BTreeMap::<
            RegionKey,
            (
                RegionLease,
                Vec<CompactEntityKinematicsFence>,
                Vec<EntityKinematics>,
            ),
        >::new();
        for ((previous, lease), state) in
            expected.expected.iter().zip(&expected.routes).zip(&states)
        {
            let lease = *lease;
            let duplicate = !ids.insert(state.id);
            if !previous.eligible()
                || previous.id != state.id
                || !state.is_finite()
                || duplicate
                || self.locations.get(&state.id).copied() != Some(lease.key)
                || self.ownership.lease(lease.key) != Some(lease)
                || RegionKey::from_position(previous.expected.position) != Some(lease.key)
                || RegionKey::from_position(state.position) != Some(lease.key)
                || self.vehicle_passengers.contains_key(&state.id)
            {
                #[cfg(feature = "load-bench")]
                eprintln!(
                    "PHYSICS_VERSIONED_REJECT reason=validation expected={} states={} id={:?} eligible={} id_match={} finite={} duplicate={} location={} ownership={} previous_region={} state_region={} passenger={}",
                    expected.expected.len(),
                    states.len(),
                    state.id,
                    previous.eligible(),
                    previous.id == state.id,
                    state.is_finite(),
                    duplicate,
                    self.locations.get(&state.id).copied() == Some(lease.key),
                    self.ownership.lease(lease.key) == Some(lease),
                    RegionKey::from_position(previous.expected.position) == Some(lease.key),
                    RegionKey::from_position(state.position) == Some(lease.key),
                    self.vehicle_passengers.contains_key(&state.id)
                );
                return Ok(None);
            }
            let (_, previous_states, next_states) = by_region
                .entry(lease.key)
                .or_insert_with(|| (lease, Vec::new(), Vec::new()));
            previous_states.push(*previous);
            next_states.push(*state);
        }
        let lanes = expected
            .routes
            .iter()
            .map(|lease| lease.lane)
            .collect::<BTreeSet<_>>();
        if !lanes.iter().eq(expected.fence.lane_versions.keys()) {
            #[cfg(feature = "load-bench")]
            eprintln!(
                "PHYSICS_VERSIONED_REJECT reason=lanes expected={} states={}",
                expected.expected.len(),
                states.len()
            );
            return Ok(None);
        }
        #[cfg(feature = "load-bench")]
        let validation_us = validation_started.elapsed().as_micros();
        #[cfg(feature = "load-bench")]
        let setup_started = std::time::Instant::now();
        let before_lane_versions = expected.fence.lane_versions.clone();
        let (first_sequence, next_sequence) =
            self.commit_state.reserve_sequences(by_region.len())?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (_key, (lease, previous, states))) in by_region.into_iter().enumerate() {
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetKinematicsBatchIfVersion {
                        expected_state_version: expected.fence.lane_versions[&lease.lane],
                        previous,
                        states,
                    },
                });
        }
        #[cfg(feature = "load-bench")]
        let setup_us = setup_started.elapsed().as_micros();
        #[cfg(feature = "load-bench")]
        let execution_started = std::time::Instant::now();
        let execution = self.execute_mutations_with_stats(mutations, next_sequence, journal_commit);
        match execution {
            Ok(_) => {
                #[cfg(feature = "load-bench")]
                let reconstruction_started = std::time::Instant::now();
                let mut lane_versions = BTreeMap::new();
                for lane in expected.fence.lane_versions.keys() {
                    let Some(owner) = self.lanes.get(lane) else {
                        return Err(RegionOwnerLaneError::WrongLane);
                    };
                    lane_versions.insert(*lane, owner.reader().state_version());
                }
                #[cfg(feature = "load-bench")]
                eprintln!(
                    "PHYSICS_VERSIONED_COMMIT_PROFILE expected={} validation_us={} setup_us={} execute_us={} reconstruction_us={}",
                    expected.expected.len(),
                    validation_us,
                    setup_us,
                    execution_started.elapsed().as_micros(),
                    reconstruction_started.elapsed().as_micros()
                );
                Ok(Some(VersionedKinematicsCommit {
                    states,
                    fence: VersionedEntitySnapshots {
                        authority: self.authority,
                        fence: RegionalVersionFence {
                            leases: expected
                                .expected
                                .iter()
                                .zip(&expected.routes)
                                .map(|(previous, lease)| (previous.id, *lease))
                                .collect(),
                            lane_versions,
                        },
                        snapshots: Vec::new(),
                        publication_readers: None,
                    },
                    before_lane_versions,
                }))
            }
            Err(RegionOwnerLaneError::InvalidMutation) => {
                #[cfg(feature = "load-bench")]
                eprintln!(
                    "PHYSICS_VERSIONED_REJECT reason=execution expected={} states={}",
                    expected.expected.len(),
                    states.len()
                );
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    fn apply_local_kinematics_if_versioned(
        &mut self,
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
    ) -> Result<Option<VersionedKinematicsCommit>, RegionOwnerLaneError> {
        let mut by_lane = BTreeMap::<usize, BTreeMap<RegionKey, LocalKinematicsRegionBatch>>::new();
        for ((previous, lease), state) in
            expected.expected.iter().zip(&expected.routes).zip(&states)
        {
            let lease = *lease;
            let batch = by_lane
                .entry(lease.lane)
                .or_default()
                .entry(lease.key)
                .or_insert_with(|| LocalKinematicsRegionBatch {
                    lease,
                    previous: Vec::new(),
                    states: Vec::new(),
                });
            batch.previous.push(*previous);
            batch.states.push(*state);
        }

        let mut pending = Vec::with_capacity(by_lane.len());
        for (lane, regions) in by_lane {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            pending.push((
                lane,
                owner.apply_local_kinematics_if_version(regions.into_values().collect())?,
            ));
        }

        let mut committed_states = Vec::with_capacity(states.len());
        let mut committed_leases = HashMap::new();
        let mut lane_versions = BTreeMap::new();
        let routes_by_id = expected
            .expected
            .iter()
            .zip(&expected.routes)
            .map(|(previous, route)| (previous.id, *route))
            .collect::<HashMap<_, _>>();
        for (lane, completion) in pending {
            let Some(commit) = completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??
            else {
                continue;
            };
            lane_versions.insert(lane, commit.state_version);
            for state in commit.states {
                let lease = *routes_by_id
                    .get(&state.id)
                    .expect("committed kinematics were prevalidated");
                committed_leases.insert(state.id, lease);
                committed_states.push(state);
            }
        }
        committed_states.sort_unstable_by_key(|state| state.id);
        if committed_states.is_empty() {
            return Ok(None);
        }
        let mut before_lane_versions = expected.fence.lane_versions;
        before_lane_versions.retain(|lane, _| lane_versions.contains_key(lane));
        Ok(Some(VersionedKinematicsCommit {
            states: committed_states,
            fence: VersionedEntitySnapshots {
                authority: self.authority,
                fence: RegionalVersionFence {
                    leases: committed_leases,
                    lane_versions,
                },
                snapshots: Vec::new(),
                publication_readers: None,
            },
            before_lane_versions,
        }))
    }

    pub(super) fn apply_kinematics_if_current_inner(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, EntityKinematics)>,
        journal_commit: bool,
    ) -> Result<bool, RegionOwnerLaneError> {
        let mut states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return Ok(true);
        }
        states.sort_unstable_by_key(|(expected, _)| expected.id);
        let mut ids = HashSet::with_capacity(states.len());
        for (expected, state) in &states {
            if expected.id != state.id
                || RegionKey::from_position(expected.position).is_none()
                || !state.is_finite()
                || !ids.insert(expected.id)
            {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if expected.retained.item_pickup_claim.is_some() && state.position != expected.position
            {
                return Ok(false);
            }
        }
        if ids.iter().any(|id| !self.locations.contains_key(id)) {
            return Ok(false);
        }
        let standalone_local = states.iter().all(|(expected, state)| {
            let source = RegionKey::from_position(expected.position);
            self.locations.get(&expected.id).copied() == source
                && RegionKey::from_position(state.position) == source
                && !self.vehicle_passengers.contains_key(&expected.id)
                && !self.passenger_vehicles.contains_key(&expected.id)
                && snapshot_vehicle_reference(expected).is_none()
        });
        if standalone_local {
            let mut states_by_region =
                BTreeMap::<RegionKey, Vec<(EntitySnapshot, EntityKinematics)>>::new();
            for (expected, state) in states {
                let source = RegionKey::from_position(expected.position)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                states_by_region
                    .entry(source)
                    .or_default()
                    .push((expected, state));
            }
            let (first_sequence, next_sequence) = self
                .commit_state
                .reserve_sequences(states_by_region.len())?;
            let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
            for (offset, (source, states)) in states_by_region.into_iter().enumerate() {
                let lease = self
                    .ownership
                    .lease(source)
                    .ok_or(RegionOwnerLaneError::StaleLease)?;
                let (expected, states) = states.into_iter().unzip();
                mutations
                    .entry(lease.lane)
                    .or_default()
                    .push(SequencedRegionMutation {
                        sequence: first_sequence + offset as u64 + 1,
                        lease,
                        mutation: RegionOwnerMutation::SetKinematicsBatchIfCurrent {
                            expected,
                            states,
                        },
                    });
            }
            return match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit)
            {
                Ok(_) => Ok(true),
                Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
                Err(error) => Err(error),
            };
        }
        let component_ids = self.vehicle_component_ids(&ids)?;
        let snapshots = if component_ids.len() == self.locations.len() {
            self.snapshots()?
        } else {
            self.snapshots_for_ids(&component_ids)?
        };
        let current = snapshots
            .into_iter()
            .map(|snapshot| (snapshot.id, snapshot))
            .collect::<BTreeMap<_, _>>();
        if current.len() != component_ids.len()
            || current.values().any(|snapshot| {
                snapshot_vehicle_reference(snapshot)
                    != self.vehicle_passengers.get(&snapshot.id).copied()
                    || self
                        .passenger_vehicles
                        .get(&snapshot.id)
                        .is_some_and(|vehicle| {
                            self.vehicle_passengers.get(vehicle).copied() != Some(snapshot.id)
                        })
            })
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if states
            .iter()
            .any(|(expected, _)| current.get(&expected.id) != Some(expected))
        {
            return Ok(false);
        }
        let input = states
            .iter()
            .map(|(expected, state)| (expected.id, (expected, state)))
            .collect::<HashMap<_, _>>();
        let parents = current
            .values()
            .filter_map(|snapshot| {
                snapshot_vehicle_reference(snapshot).map(|passenger| (passenger, snapshot.id))
            })
            .collect::<HashMap<_, _>>();
        let mut handled = HashSet::new();
        let mut plans = Vec::new();
        let mut target_regions = BTreeSet::new();
        for (expected, state) in &states {
            if handled.contains(&expected.id) {
                continue;
            }
            let source = self.locations[&expected.id];
            let mut leader = expected.id;
            while let Some(parent) = parents.get(&leader).copied() {
                leader = parent;
            }
            let mut group = Vec::new();
            let mut member = Some(leader);
            while let Some(entity) = member {
                let snapshot = current
                    .get(&entity)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                if self.locations.get(&entity).copied() != Some(source) {
                    return Err(RegionOwnerLaneError::InvalidMutation);
                }
                group.push(snapshot.clone());
                member = snapshot_vehicle_reference(snapshot);
            }
            let leader_state = input.get(&leader).map(|(_, state)| **state);
            let target = leader_state
                .and_then(|state| RegionKey::from_position(state.position))
                .unwrap_or(source);
            if group.len() > 1 && target != source {
                let leader_state = leader_state.ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let leader_snapshot = current
                    .get(&leader)
                    .ok_or(RegionOwnerLaneError::InvalidMutation)?;
                let delta = Vec3::new(
                    leader_state.position.x - leader_snapshot.position.x,
                    leader_state.position.y - leader_snapshot.position.y,
                    leader_state.position.z - leader_snapshot.position.z,
                );
                let mut moved = Vec::with_capacity(group.len());
                for snapshot in &group {
                    let mut target_snapshot = snapshot.clone();
                    if snapshot.id == leader {
                        target_snapshot.position = leader_state.position;
                        target_snapshot.rotation = leader_state.rotation;
                        target_snapshot.velocity = leader_state.velocity;
                        target_snapshot.on_ground = leader_state.on_ground;
                    } else {
                        target_snapshot.position = Vec3::new(
                            snapshot.position.x + delta.x,
                            snapshot.position.y + delta.y,
                            snapshot.position.z + delta.z,
                        );
                    }
                    if RegionKey::from_position(target_snapshot.position) != Some(target) {
                        return Ok(false);
                    }
                    moved.push(target_snapshot);
                }
                target_regions.insert(target);
                handled.extend(group.iter().map(|snapshot| snapshot.id));
                plans.push(OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected: group,
                    moved,
                });
                continue;
            }

            let target = RegionKey::from_position(state.position)
                .ok_or(RegionOwnerLaneError::InvalidMutation)?;
            if group.len() > 1 && target != source {
                return Ok(false);
            }
            handled.insert(expected.id);
            if source == target {
                plans.push(OwnerKinematicsPlan::Local {
                    source,
                    expected: Box::new(expected.clone()),
                    state: *state,
                });
            } else {
                let mut moved = expected.clone();
                moved.position = state.position;
                moved.rotation = state.rotation;
                moved.velocity = state.velocity;
                moved.on_ground = state.on_ground;
                target_regions.insert(target);
                plans.push(OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected: vec![expected.clone()],
                    moved: vec![moved],
                });
            }
        }
        for target in target_regions {
            self.ensure_region(target)?;
        }
        let mutation_count = plans
            .iter()
            .map(|plan| match plan {
                OwnerKinematicsPlan::Local { .. } => 1usize,
                OwnerKinematicsPlan::Migrate { .. } => 2usize,
            })
            .try_fold(0usize, usize::checked_add)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let (first_sequence, next_sequence) =
            self.commit_state.reserve_sequences(mutation_count)?;
        let mut mutations = BTreeMap::new();
        let mut migrations = Vec::new();
        let mut sequence = first_sequence;
        for plan in plans {
            match plan {
                OwnerKinematicsPlan::Local {
                    source,
                    expected,
                    state,
                } => {
                    let lease = self
                        .ownership
                        .lease(source)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    mutations.entry(lease.lane).or_insert_with(Vec::new).push(
                        SequencedRegionMutation {
                            sequence,
                            lease,
                            mutation: RegionOwnerMutation::SetKinematicsIfCurrent {
                                expected,
                                state,
                            },
                        },
                    );
                }
                OwnerKinematicsPlan::Migrate {
                    source,
                    target,
                    expected,
                    moved,
                } => {
                    let source_lease = self
                        .ownership
                        .lease(source)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    mutations
                        .entry(source_lease.lane)
                        .or_insert_with(Vec::new)
                        .push(SequencedRegionMutation {
                            sequence,
                            lease: source_lease,
                            mutation: RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected),
                        });
                    let target_lease = self
                        .ownership
                        .lease(target)
                        .ok_or(RegionOwnerLaneError::StaleLease)?;
                    sequence += 1;
                    migrations.extend(moved.iter().map(|snapshot| (snapshot.id, target)));
                    mutations
                        .entry(target_lease.lane)
                        .or_insert_with(Vec::new)
                        .push(SequencedRegionMutation {
                            sequence,
                            lease: target_lease,
                            mutation: RegionOwnerMutation::InsertSnapshots(moved),
                        });
                }
            }
        }
        debug_assert_eq!(sequence, next_sequence);
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(_) => {
                for (entity, target) in migrations {
                    self.locations.insert(entity, target);
                }
                Ok(true)
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(false),
            Err(error) => Err(error),
        }
    }

    pub fn damage_if_current(
        &mut self,
        expected: EntitySnapshot,
        request: EntityDamageRequest,
    ) -> Result<Option<EntityDamage>, RegionOwnerLaneError> {
        if !request.is_valid() {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        if expected.retained.item_pickup_claim.is_some() {
            return Ok(None);
        }
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(None);
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let entity = expected.id;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::DamageIfCurrent {
                    expected: Box::new(expected),
                    request,
                },
            }],
        )]);
        match self.execute_mutations(mutations, sequence) {
            Ok(()) => {
                let snapshot = self
                    .snapshot(entity)?
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                Ok(Some(EntityDamage {
                    killed: snapshot.lifecycle == crate::EntityLifecycle::Despawning,
                    snapshot,
                }))
            }
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn apply_effect_if_current(
        &mut self,
        expected: EntitySnapshot,
        request: EntityEffectRequest,
    ) -> Result<EntityEffectResult, RegionOwnerLaneError> {
        let Some(key) = RegionKey::from_position(expected.position) else {
            return Err(RegionOwnerLaneError::InvalidMutation);
        };
        if self.locations.get(&expected.id).copied() != Some(key) {
            return Ok(EntityEffectResult::Rejected(
                crate::EntityEffectRejection::Missing,
            ));
        }
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let (_, sequence) = self.commit_state.reserve_sequences(1)?;
        let mutations = BTreeMap::from([(
            lease.lane,
            vec![SequencedRegionMutation {
                sequence,
                lease,
                mutation: RegionOwnerMutation::ApplyEffectIfCurrent {
                    expected: Box::new(expected),
                    request: Box::new(request),
                },
            }],
        )]);
        match self.execute_mutations_with_stats(mutations, sequence, true) {
            Ok(mut execution) => execution
                .effect_results
                .pop()
                .map(|(_, result)| result)
                .ok_or(RegionOwnerLaneError::InvalidMutation),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(EntityEffectResult::Rejected(
                crate::EntityEffectRejection::Stale,
            )),
            Err(error) => Err(error),
        }
    }

    pub fn set_velocities(
        &mut self,
        velocities: impl IntoIterator<Item = (EntityId, Vec3)>,
    ) -> Result<(), RegionOwnerLaneError> {
        let velocities = velocities.into_iter().collect::<Vec<_>>();
        if velocities.is_empty() {
            return Ok(());
        }
        let mut ids = HashSet::with_capacity(velocities.len());
        for (entity, velocity) in &velocities {
            if !ids.insert(*entity) || !velocity.is_finite() {
                return Err(RegionOwnerLaneError::InvalidMutation);
            }
            if !self.locations.contains_key(entity) {
                return Err(RegionOwnerLaneError::UnknownEntity);
            }
        }
        let (first_sequence, next_sequence) =
            self.commit_state.reserve_sequences(velocities.len())?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (entity, velocity)) in velocities.into_iter().enumerate() {
            let key = self.locations[&entity];
            let lease = self.ownership.lease(key).expect("indexed entity lease");
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::SetVelocity { entity, velocity },
                });
        }
        self.execute_mutations(mutations, next_sequence)
    }

    pub(super) fn tick_owned_regions(
        &self,
        input: RegionalEntityTickInput,
    ) -> Result<RegionalEntityTickOutput, RegionOwnerLaneError> {
        let readers = self
            .lanes
            .iter()
            .map(|(&lane, owner)| (lane, owner.reader()))
            .collect::<Vec<_>>();
        // Prevent a multi-lane transaction from landing between tick fanout
        // messages, but release admissions before waiting for lane work.
        let admissions = readers
            .iter()
            .map(|(_, reader)| {
                lock_authoritative_mutex(reader.admission(), "regional.owner_lane_admission")
            })
            .collect::<Vec<_>>();
        let mut pending = Vec::with_capacity(readers.len());
        for (lane, reader) in &readers {
            pending.push((
                *lane,
                reader.clone(),
                reader.tick_owned_regions(input.clone())?,
            ));
        }
        drop(admissions);

        let mut goal_stats = GoalTickStats::default();
        let mut active_entity_count = 0usize;
        let mut active_hostile_ids = Vec::new();
        let mut villager_population_candidates = Vec::new();
        let mut villager_ids = Vec::new();
        let mut villager_proximity_seeds = Vec::new();
        let mut fallback_entity_ids = HashSet::new();
        let mut resolved_direct_paths = HashSet::new();
        let mut villager_profession_updates = Vec::new();
        let mut prepared_lanes = Vec::new();
        for (lane, owner, completed) in pending {
            let output = completed.recv().map_err(|_| owner.unavailable_error())??;
            add_goal_tick_stats(&mut goal_stats, output.goal_stats);
            active_entity_count = active_entity_count.saturating_add(output.active_entity_count);
            active_hostile_ids.extend(output.active_hostile_ids);
            villager_population_candidates.extend(output.villager_population_candidates);
            villager_ids.extend(output.villager_ids);
            villager_proximity_seeds.extend(output.villager_proximity_seeds);
            fallback_entity_ids.extend(output.fallback_entity_ids);
            resolved_direct_paths.extend(output.resolved_direct_paths);
            villager_profession_updates.extend(output.villager_profession_updates);
            if !output.physics_regions.is_empty() || !output.goal_committed_motion.is_empty() {
                for (lease, motion) in &output.goal_committed_motion {
                    if lease.lane != lane
                        || self.ownership.lease(lease.key) != Some(*lease)
                        || RegionKey::from_position(motion.position) != Some(lease.key)
                    {
                        return Err(RegionOwnerLaneError::StaleLease);
                    }
                }
                prepared_lanes.push(RegionalPreparedLanePhysics {
                    lane,
                    regions: output.physics_regions,
                    goal_committed_motion: output.goal_committed_motion,
                });
            }
        }
        active_hostile_ids.sort_unstable_by_key(|(entity, _)| *entity);
        villager_population_candidates.sort_unstable_by_key(|(entity, _)| *entity);
        villager_ids.sort_unstable_by_key(|(entity, _)| *entity);
        villager_proximity_seeds.sort_unstable_by_key(|(entity, _)| *entity);
        villager_profession_updates.sort_unstable_by_key(|snapshot| snapshot.id);
        villager_profession_updates.dedup_by_key(|snapshot| snapshot.id);

        Ok(RegionalEntityTickOutput {
            goal_stats,
            active_entity_count,
            active_hostile_ids,
            villager_population_candidates,
            villager_ids,
            villager_proximity_seeds,
            fallback_entity_ids,
            prepared_physics: RegionalPreparedEntityPhysics {
                authority: self.authority,
                simulation_chunks: input.simulation_chunks,
                world: input.world,
                lanes: prepared_lanes,
            },
            resolved_direct_paths,
            villager_profession_updates,
        })
    }

    pub(super) fn commit_owned_region_physics(
        &self,
        prepared: RegionalPreparedEntityPhysics,
    ) -> Result<RegionalEntityPhysicsOutput, RegionOwnerLaneError> {
        self.commit_state.ensure_committed_state()?;
        if prepared.authority != self.authority {
            return Err(RegionOwnerLaneError::StaleLease);
        }
        let readers = prepared
            .lanes
            .iter()
            .map(|lane| {
                self.lanes
                    .get(&lane.lane)
                    .map(|owner| (lane.lane, owner.reader()))
                    .ok_or(RegionOwnerLaneError::WrongLane)
            })
            .collect::<Result<Vec<_>, _>>()?;
        let admission_started = std::time::Instant::now();
        let admissions = readers
            .iter()
            .map(|(_, reader)| {
                lock_authoritative_mutex(reader.admission(), "regional.owner_lane_admission")
            })
            .collect::<Vec<_>>();
        let admission_wait_us = admission_started
            .elapsed()
            .as_micros()
            .min(u128::from(u64::MAX)) as u64;
        let mut pending = Vec::with_capacity(readers.len());
        for (plan, (lane, reader)) in prepared.lanes.into_iter().zip(&readers) {
            if plan.lane != *lane {
                return Err(RegionOwnerLaneError::WrongLane);
            }
            let completed =
                reader.tick_owned_region_physics(owner_lane::OwnerLanePhysicsTickInput {
                    regions: plan.regions,
                    goal_motion: plan.goal_committed_motion,
                    simulation_chunks: Arc::clone(&prepared.simulation_chunks),
                    world: Arc::clone(&prepared.world),
                })?;
            pending.push((*lane, reader.clone(), completed));
        }
        drop(admissions);

        let mut fallback_candidates = Vec::new();
        let mut committed_motion = Vec::new();
        let mut terrain_pathing_additions = HashSet::new();
        let mut lane_versions = BTreeMap::new();
        let mut owner_readers = BTreeMap::new();
        let mut publication_lanes = BTreeSet::new();
        let mut worker_wait_us = 0;
        let mut worker_exec_us = 0;
        for (lane, owner, completed) in pending {
            let worker_started = std::time::Instant::now();
            let output = completed.recv().map_err(|_| owner.unavailable_error())??;
            worker_exec_us = worker_exec_us.max(output.worker_exec_us);
            worker_wait_us += worker_started
                .elapsed()
                .as_micros()
                .min(u128::from(u64::MAX)) as u64;
            let lane_motion = output.committed_motion;
            for (lease, motion) in &lane_motion {
                if lease.lane != lane
                    || self.ownership.lease(lease.key) != Some(*lease)
                    || RegionKey::from_position(motion.position) != Some(lease.key)
                {
                    return Err(RegionOwnerLaneError::StaleLease);
                }
            }
            if !lane_motion.is_empty() || !output.fallback_candidates.is_empty() {
                lane_versions.insert(lane, output.state_version);
            }
            if !lane_motion.is_empty() {
                publication_lanes.insert(lane);
            }
            owner_readers.insert(lane, owner);
            fallback_candidates.extend(output.fallback_candidates);
            committed_motion.extend(lane_motion.into_iter().map(|(_, motion)| motion));
            terrain_pathing_additions.extend(output.terrain_pathing_additions);
        }
        fallback_candidates.sort_unstable_by_key(|candidate| candidate.id);

        let publication_fence = VersionedEntitySnapshots {
            authority: self.authority,
            fence: RegionalVersionFence {
                leases: HashMap::new(),
                lane_versions: publication_lanes
                    .iter()
                    .filter_map(|lane| lane_versions.get(lane).map(|&version| (*lane, version)))
                    .collect(),
            },
            snapshots: Vec::new(),
            publication_readers: Some(Arc::new(
                publication_lanes
                    .into_iter()
                    .filter_map(|lane| owner_readers.get(&lane).cloned().map(|owner| (lane, owner)))
                    .collect(),
            )),
        };
        let fallback_results = fallback_candidates
            .iter()
            .filter_map(|candidate| candidate.result)
            .collect::<Vec<_>>();
        let fallback_fence = if fallback_candidates.is_empty() {
            None
        } else {
            let routes = fallback_candidates
                .iter()
                .map(|candidate| candidate.lease)
                .collect::<Vec<_>>();
            let expected = fallback_candidates
                .iter()
                .filter_map(|candidate| candidate.expected())
                .collect::<Vec<_>>();
            if expected.len() != routes.len() {
                return Err(RegionOwnerLaneError::InvalidQuery);
            }
            let leases = fallback_candidates
                .iter()
                .map(|candidate| (candidate.id, candidate.lease))
                .collect::<HashMap<_, _>>();
            let selected_lanes = routes
                .iter()
                .map(|lease| lease.lane)
                .collect::<BTreeSet<_>>();
            Some(VersionedEntityKinematics {
                authority: self.authority,
                fence: RegionalVersionFence {
                    leases,
                    lane_versions: selected_lanes
                        .into_iter()
                        .filter_map(|lane| lane_versions.get(&lane).map(|&version| (lane, version)))
                        .collect(),
                },
                expected,
                routes,
                motion_states: None,
                owner_readers: Some(Arc::new(owner_readers)),
            })
        };
        Ok(RegionalEntityPhysicsOutput {
            fallback_results,
            fallback_fence,
            committed_motion,
            publication_fence,
            terrain_pathing_additions,
            lane_timings: LaneCommitTimings {
                admission_wait_us,
                worker_wait_us,
                worker_exec_us,
                exec_us: 0,
            },
        })
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        self.prepare_goal_tick_with_pathing_for_ids_from_snapshots(
            tick,
            active_ids,
            None,
            RegionalGoalTickInputs::default(),
        )
    }

    pub(super) fn prepare_goal_tick_with_pathing_for_ids_from_snapshots(
        &self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
        _selected: Option<(Vec<EntitySnapshot>, BTreeMap<usize, u64>)>,
        inputs: RegionalGoalTickInputs,
    ) -> Result<RegionalPreparedGoalTick, RegionOwnerLaneError> {
        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            let key = self
                .locations
                .get(id)
                .copied()
                .ok_or(RegionOwnerLaneError::UnknownEntity)?;
            ids_by_region.entry(key).or_default().insert(*id);
        }

        let mut goal_inputs = BTreeMap::<RegionKey, Vec<EntityGoalCheckpoint>>::new();
        let mut goal_overrides = BTreeMap::new();
        let mut snapshot_overrides = BTreeMap::new();
        let mut villager_updates = BTreeMap::new();
        let mut cross_region_villager_candidates = HashSet::new();
        let mut pathing_aabbs = HashMap::new();
        let mut lane_versions = BTreeMap::new();
        let mut pending = Vec::with_capacity(ids_by_region.len());
        for (&key, ids) in &ids_by_region {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            let owner = self
                .lanes
                .get(&lease.lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let (current_version, completion) =
                owner.request_goal_tick_admitted(lease, tick, ids.clone(), inputs.clone())?;
            if lane_versions
                .insert(lease.lane, current_version)
                .is_some_and(|existing| existing != current_version)
            {
                return Err(RegionOwnerLaneError::Busy);
            }
            pending.push((key, lease.lane, completion));
        }

        let mut batches = BTreeMap::new();
        for (key, lane, completion) in pending {
            let prepared = completion
                .recv()
                .map_err(|_| RegionOwnerLaneError::Closed)??;
            let current_version = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?
                .reader()
                .state_version();
            if lane_versions.get(&lane).copied() != Some(current_version) {
                return Err(RegionOwnerLaneError::Busy);
            }
            if goal_inputs.insert(key, prepared.checkpoints).is_some() {
                return Err(RegionOwnerLaneError::Busy);
            }
            if !prepared.goal_overrides.is_empty() {
                goal_overrides.insert(key, prepared.goal_overrides);
            }
            if !prepared.snapshot_overrides.is_empty() {
                snapshot_overrides.insert(key, prepared.snapshot_overrides);
            }
            if !prepared.villager_updates.is_empty() {
                villager_updates.insert(key, prepared.villager_updates);
            }
            cross_region_villager_candidates.extend(prepared.cross_region_villager_candidates);
            for (entity, aabb) in prepared.pathing_aabbs {
                if pathing_aabbs.insert(entity, aabb).is_some() {
                    return Err(RegionOwnerLaneError::Busy);
                }
            }
            batches.insert(key, prepared.goal_tick);
        }

        let referenced_target_ids = goal_inputs
            .values()
            .flatten()
            .filter_map(|checkpoint| goal_reference(&checkpoint.goal))
            .collect::<HashSet<_>>();
        let mut target_checkpoints = goal_inputs
            .values()
            .flatten()
            .filter(|checkpoint| referenced_target_ids.contains(&checkpoint.id))
            .map(|checkpoint| (checkpoint.id, checkpoint.clone()))
            .collect::<HashMap<_, _>>();
        let target_ids = referenced_target_ids
            .iter()
            .copied()
            .filter(|target| !target_checkpoints.contains_key(target))
            .filter(|target| self.locations.contains_key(target))
            .collect::<HashSet<_>>();
        if !target_ids.is_empty() {
            let (targets, target_versions) = self.goal_checkpoints_for_ids_admitted(&target_ids)?;
            for (lane, version) in target_versions {
                if lane_versions
                    .insert(lane, version)
                    .is_some_and(|existing| existing != version)
                {
                    return Err(RegionOwnerLaneError::Busy);
                }
            }
            target_checkpoints.extend(
                targets
                    .into_iter()
                    .map(|checkpoint| (checkpoint.id, checkpoint)),
            );
        }
        let expected_missing_follow_targets = referenced_target_ids
            .into_iter()
            .filter(|target| !target_checkpoints.contains_key(target))
            .collect::<HashSet<_>>();

        let mut follow_targets = BTreeMap::<RegionKey, HashMap<EntityId, Vec3>>::new();
        let mut follow_target_sources = BTreeMap::new();
        for (&region, followers) in &goal_inputs {
            for follower in followers {
                let Some(target) = goal_reference(&follower.goal) else {
                    continue;
                };
                let Some(target_region) = self.locations.get(&target).copied() else {
                    continue;
                };
                let target_checkpoint = target_checkpoints
                    .get(&target)
                    .ok_or(RegionOwnerLaneError::UnknownEntity)?;
                follow_target_sources.insert(
                    target,
                    RegionalFollowTargetSource {
                        region: target_region,
                        checkpoint: target_checkpoint.clone(),
                    },
                );
                follow_targets
                    .entry(region)
                    .or_default()
                    .insert(target, target_checkpoint.position);
            }
        }
        let simulation_ids = ids_by_region
            .into_iter()
            .map(|(key, ids)| {
                let mut ids = ids.into_iter().collect::<Vec<_>>();
                ids.sort_unstable();
                (key, ids)
            })
            .collect();
        let lease_keys = batches
            .keys()
            .copied()
            .chain(follow_target_sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        Ok(RegionalPreparedGoalTick {
            authority: self.authority,
            phase: RegionPhase(0),
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    self.ownership
                        .lease(key)
                        .map(|lease| (key, lease))
                        .ok_or(RegionOwnerLaneError::StaleLease)
                })
                .collect::<Result<_, _>>()?,
            lane_versions,
            simulation_ids,
            batches,
            follow_targets,
            follow_target_sources,
            expected_missing_follow_targets,
            goal_inputs,
            goal_overrides,
            snapshot_overrides,
            villager_updates,
            cross_region_villager_candidates,
            pathing_aabbs,
        })
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        self.apply_prepared_goal_tick_inner(resolved, true)
    }

    fn apply_prepared_goal_tick_inner(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        journal_commit: bool,
    ) -> Result<GoalTickStats, RegionOwnerLaneError> {
        Ok(self
            .try_apply_prepared_goal_tick_inner(resolved, journal_commit, None)?
            .map_or_else(GoalTickStats::default, |execution| execution.goal_stats))
    }

    pub(super) fn apply_prepared_goal_tick_and_kinematics_for_ids(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        entities: &HashSet<EntityId>,
        journal_commit: bool,
    ) -> GoalApplySimulationResult {
        let Some(execution) =
            self.try_apply_prepared_goal_tick_inner(resolved, journal_commit, Some(entities))?
        else {
            return Ok(None);
        };
        let (_, results, versioned) = self.fenced_simulation_results_from_candidate_runs(
            entities,
            execution.goal_candidate_runs,
            execution.lane_versions,
            None,
            true,
        )?;
        Ok(Some((execution.goal_stats, results, versioned)))
    }

    fn try_apply_prepared_goal_tick_inner(
        &mut self,
        mut resolved: RegionalResolvedGoalTick,
        journal_commit: bool,
        simulation_entities: Option<&HashSet<EntityId>>,
    ) -> Result<Option<MutationExecution>, RegionOwnerLaneError> {
        if resolved.authority != self.authority || resolved.phase != RegionPhase(0) {
            return Ok(None);
        }
        if resolved
            .leases
            .iter()
            .any(|(key, lease)| lease.key != *key || !self.ownership.validate(*lease))
        {
            return Ok(None);
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.locations.contains_key(target))
        {
            return Ok(None);
        }
        if resolved.lane_versions.iter().any(|(lane, version)| {
            self.lanes
                .get(lane)
                .is_none_or(|owner| owner.reader().state_version() != *version)
        }) {
            return Ok(None);
        }

        let validation_ids = resolved
            .follow_target_sources
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        let (current, _) = self.goal_checkpoints_for_ids_fenced(&validation_ids)?;
        let current = current
            .into_iter()
            .map(|checkpoint| (checkpoint.id, checkpoint))
            .collect::<BTreeMap<_, _>>();
        if resolved.follow_target_sources.iter().any(|(id, expected)| {
            self.locations.get(id).copied() != Some(expected.region)
                || current.get(id) != Some(&expected.checkpoint)
        }) {
            return Ok(None);
        }
        if resolved.batches.is_empty() {
            return Ok(None);
        }
        let mut candidate_ids_by_region = if let Some(entities) = simulation_entities {
            let prepared_len = resolved
                .simulation_ids
                .values()
                .map(Vec::len)
                .sum::<usize>();
            if prepared_len != entities.len()
                || resolved
                    .simulation_ids
                    .values()
                    .flatten()
                    .any(|entity| !entities.contains(entity))
            {
                return Ok(None);
            }
            std::mem::take(&mut resolved.simulation_ids)
        } else {
            BTreeMap::new()
        };

        let mut expected_by_region = std::mem::take(&mut resolved.goal_inputs);
        let batch_count = resolved.batches.len();
        let (first_sequence, next_sequence) = self.commit_state.reserve_sequences(batch_count)?;
        let mut mutations = BTreeMap::<usize, Vec<SequencedRegionMutation>>::new();
        for (offset, (key, batch)) in resolved.batches.into_iter().enumerate() {
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionOwnerLaneError::StaleLease)?;
            let expected = expected_by_region.remove(&key).unwrap_or_default();
            mutations
                .entry(lease.lane)
                .or_default()
                .push(SequencedRegionMutation {
                    sequence: first_sequence + offset as u64 + 1,
                    lease,
                    mutation: RegionOwnerMutation::ApplyGoalBatch {
                        expected,
                        candidate_ids: candidate_ids_by_region.remove(&key).unwrap_or_default(),
                        expected_state_version: resolved
                            .lane_versions
                            .get(&lease.lane)
                            .copied()
                            .ok_or(RegionOwnerLaneError::WrongLane)?,
                        resolved: Box::new(batch),
                        follow_targets: resolved.follow_targets.remove(&key).unwrap_or_default(),
                        goal_overrides: resolved.goal_overrides.remove(&key).unwrap_or_default(),
                        snapshot_overrides: resolved
                            .snapshot_overrides
                            .remove(&key)
                            .unwrap_or_default(),
                        villager_updates: resolved
                            .villager_updates
                            .remove(&key)
                            .unwrap_or_default(),
                    },
                });
        }
        match self.execute_mutations_with_stats(mutations, next_sequence, journal_commit) {
            Ok(execution) => Ok(Some(execution)),
            Err(RegionOwnerLaneError::InvalidMutation) => Ok(None),
            Err(error) => Err(error),
        }
    }

    fn execute_mutations(
        &mut self,
        mutations: BTreeMap<usize, Vec<SequencedRegionMutation>>,
        sequence_watermark: u64,
    ) -> Result<(), RegionOwnerLaneError> {
        self.execute_mutations_with_stats(mutations, sequence_watermark, true)
            .map(|_| ())
    }

    fn execute_mutations_with_stats(
        &mut self,
        mut mutations: BTreeMap<usize, Vec<SequencedRegionMutation>>,
        sequence_watermark: u64,
        journal_commit: bool,
    ) -> Result<MutationExecution, RegionOwnerLaneError> {
        let journal_enabled = journal_commit
            && lock_authoritative_mutex(&self.commit_state.journal, "regional.decision_journal")
                .enabled();
        let journal_entities = journal_enabled.then(|| journal_entities_by_lane(&mutations));
        let participants = mutations.keys().copied().collect::<BTreeSet<_>>();
        let phase = self.commit_state.reserve_phase()?;
        self.ownership
            .begin_allocated_phase_for_lanes(phase, participants.clone())
            .map_err(|_| RegionOwnerLaneError::Busy)?;
        let mut goal_stats = GoalTickStats::default();
        let mut goal_candidate_runs = Vec::with_capacity(participants.len());
        let mut effect_results = Vec::new();

        let mut prepared = Vec::with_capacity(participants.len());
        let mut prepared_and_committed = Vec::new();
        let mut first_error = None;
        for &lane in &participants {
            let owner = self
                .lanes
                .get(&lane)
                .ok_or(RegionOwnerLaneError::WrongLane)?;
            let batch = RegionOwnerBatch {
                phase,
                sequence_watermark,
                mutations: mutations.remove(&lane).unwrap_or_default(),
            };
            if participants.len() == 1 {
                match owner.prepare_and_commit(batch) {
                    Ok(completion) => prepared_and_committed.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            } else {
                match owner.prepare(batch) {
                    Ok(completion) => prepared.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
        }
        let mut ready = BTreeSet::new();
        let mut settled = BTreeSet::new();
        let mut applied = BTreeSet::new();
        for (lane, completion) in prepared {
            match completion.recv() {
                Ok(Ok(prepared_phase)) if prepared_phase == phase => {
                    ready.insert(lane);
                }
                Ok(Err(error)) => {
                    first_error = first_error.or(Some(error));
                    settled.insert(lane);
                }
                Ok(Ok(_)) => {
                    first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    settled.insert(lane);
                }
                Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
            }
        }
        for (lane, completion) in prepared_and_committed {
            match completion.recv() {
                Ok(Ok(result)) if result.phase == phase => {
                    add_goal_tick_stats(&mut goal_stats, result.goal_stats);
                    goal_candidate_runs.extend(result.goal_candidate_runs);
                    effect_results.extend(result.effect_results);
                    ready.insert(lane);
                    applied.insert(lane);
                }
                Ok(Err(error)) => {
                    first_error = first_error.or(Some(error));
                    settled.insert(lane);
                }
                Ok(Ok(_)) => {
                    first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    settled.insert(lane);
                }
                Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
            }
        }
        if first_error.is_none() && ready.len() == participants.len() {
            let mut committed = Vec::with_capacity(ready.len() - applied.len());
            for &lane in ready.difference(&applied) {
                match self.lanes[&lane].commit(phase) {
                    Ok(completion) => committed.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
            for (lane, completion) in committed {
                match completion.recv() {
                    Ok(Ok(result)) if result.phase == phase => {
                        add_goal_tick_stats(&mut goal_stats, result.goal_stats);
                        goal_candidate_runs.extend(result.goal_candidate_runs);
                        effect_results.extend(result.effect_results);
                        applied.insert(lane);
                    }
                    Ok(Err(error)) => {
                        first_error = first_error.or(Some(error));
                        settled.insert(lane);
                    }
                    Ok(Ok(_)) => {
                        first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
                    }
                    Err(_) => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
                }
            }
            if first_error.is_none()
                && applied.len() == participants.len()
                && let Some(entities) = journal_entities.as_ref()
                && let Err(error) = self.record_commit_decision(phase, sequence_watermark, entities)
            {
                if error.outcome_unknown() {
                    let identity = (
                        phase,
                        sequence_watermark,
                        self.commit_state.lifecycle_epoch(),
                    );
                    lock_authoritative_mutex(
                        &self.commit_state.journal_commits,
                        "regional.journal_commit_identities",
                    )
                    .insert(phase, identity);
                    self.commit_state
                        .outcome_unknown
                        .store(true, Ordering::Release);
                    return Err(RegionOwnerLaneError::Journal);
                }
                first_error = Some(RegionOwnerLaneError::Journal);
            }
            if first_error.is_none() && applied.len() == participants.len() {
                let mut finalized = Vec::with_capacity(applied.len());
                for &lane in &applied {
                    match self.lanes[&lane].finalize(phase) {
                        Ok(completion) => finalized.push((lane, completion)),
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
                for (lane, completion) in finalized {
                    match completion.recv() {
                        Ok(Ok(finalized)) if finalized == phase => {
                            settled.insert(lane);
                        }
                        Ok(Err(error)) => first_error = first_error.or(Some(error)),
                        _ => first_error = first_error.or(Some(RegionOwnerLaneError::Closed)),
                    }
                }
            } else {
                for lane in applied {
                    match self.lanes[&lane].rollback(phase) {
                        Ok(completion) => match completion.recv() {
                            Ok(Ok(rolled_back)) if rolled_back == phase => {
                                settled.insert(lane);
                            }
                            Ok(Err(error)) => first_error = first_error.or(Some(error)),
                            _ => {
                                first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                            }
                        },
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
                for lane in ready.difference(&settled).copied().collect::<Vec<_>>() {
                    match self.lanes[&lane].abort(phase) {
                        Ok(completion) => {
                            if matches!(completion.recv(), Ok(Ok(aborted)) if aborted == phase) {
                                settled.insert(lane);
                            }
                        }
                        Err(error) => first_error = first_error.or(Some(error)),
                    }
                }
            }
        } else {
            let mut rolled_back = Vec::with_capacity(applied.len());
            for &lane in &applied {
                match self.lanes[&lane].rollback(phase) {
                    Ok(completion) => rolled_back.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
            for (lane, completion) in rolled_back {
                if matches!(completion.recv(), Ok(Ok(rolled_back_phase)) if rolled_back_phase == phase)
                {
                    settled.insert(lane);
                } else {
                    first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                }
            }
            let mut aborted = Vec::with_capacity(ready.len().saturating_sub(applied.len()));
            for lane in ready.difference(&applied).copied() {
                match self.lanes[&lane].abort(phase) {
                    Ok(completion) => aborted.push((lane, completion)),
                    Err(error) => first_error = first_error.or(Some(error)),
                }
            }
            for (lane, completion) in aborted {
                if matches!(completion.recv(), Ok(Ok(aborted_phase)) if aborted_phase == phase) {
                    settled.insert(lane);
                } else {
                    first_error = first_error.or(Some(RegionOwnerLaneError::Closed));
                }
            }
        }
        let lane_versions = if first_error.is_none() {
            participants
                .iter()
                .map(|lane| {
                    self.lanes
                        .get(lane)
                        .map(|owner| (*lane, owner.reader().state_version()))
                        .ok_or(RegionOwnerLaneError::WrongLane)
                })
                .collect::<Result<_, _>>()?
        } else {
            BTreeMap::new()
        };
        for lane in settled {
            if self.ownership.acknowledge_lane(phase, lane).is_err() {
                first_error = first_error.or(Some(RegionOwnerLaneError::StalePhase));
            }
        }
        if self.ownership.finish_phase(phase).is_err() {
            return Err(first_error.unwrap_or(RegionOwnerLaneError::StalePhase));
        }
        match first_error {
            Some(error) => Err(error),
            None => Ok(MutationExecution {
                goal_stats,
                goal_candidate_runs,
                lane_versions,
                effect_results,
            }),
        }
    }

    fn record_commit_decision(
        &mut self,
        phase: RegionPhase,
        sequence_watermark: u64,
        entities_by_lane: &BTreeMap<usize, Vec<(RegionLease, EntityId)>>,
    ) -> Result<(), RegionalDecisionJournalError> {
        let mut pending = Vec::with_capacity(entities_by_lane.len());
        for (&lane, entities) in entities_by_lane {
            let snapshots = self
                .lanes
                .get(&lane)
                .ok_or(RegionalDecisionJournalError::SAFE)?
                .request_existing_snapshots_for_ids(entities.clone())
                .map_err(|_| RegionalDecisionJournalError::SAFE)?;
            pending.push(snapshots);
        }
        let affected = entities_by_lane
            .values()
            .flatten()
            .map(|(_, entity)| *entity)
            .collect::<BTreeSet<_>>();
        let mut upserts = Vec::with_capacity(affected.len());
        for snapshots in pending {
            upserts.extend(
                snapshots
                    .recv()
                    .map_err(|_| RegionalDecisionJournalError::SAFE)?
                    .map_err(|_| RegionalDecisionJournalError::SAFE)?,
            );
        }
        upserts.sort_unstable_by_key(|snapshot| snapshot.id);
        upserts.dedup_by_key(|snapshot| snapshot.id);
        let present = upserts
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<BTreeSet<_>>();
        let removed = affected.difference(&present).copied().collect();
        self.commit_state.record_commit(&RegionalCommitDecision {
            phase,
            sequence_watermark,
            lifecycle_epoch: self.commit_state.lifecycle_epoch(),
            upserts,
            removed,
        })
    }

    pub fn shutdown(self) -> Result<RegionalEntityStore, RegionalOwnerShutdownError> {
        let Self {
            authority,
            mut ownership,
            mut locations,
            mut uuids,
            vehicle_passengers: _,
            passenger_vehicles: _,
            transfers,
            in_flight_transfers,
            next_id,
            lanes,
            commit_state,
        } = self;
        ownership.last_phase = ownership
            .last_phase
            .max(commit_state.next_phase.load(Ordering::Acquire));
        let mut stores = BTreeMap::new();
        let mut first_error = commit_state.ensure_committed_state().err();
        for (_, lane) in lanes {
            match lane.shutdown() {
                Ok(owned) => {
                    for (key, store) in owned {
                        if stores.insert(key, store).is_some() {
                            first_error =
                                first_error.or(Some(RegionOwnerLaneError::DuplicateRegion));
                        }
                    }
                }
                Err(error) => first_error = first_error.or(Some(error)),
            }
        }
        if first_error.is_some() {
            retain_recovered_regional_state(&mut ownership, &stores, &mut locations, &mut uuids);
        }
        let recovered = RegionalEntityStore {
            authority,
            ownership,
            stores,
            locations,
            uuids,
            transfers,
            in_flight_transfers,

            next_id,
        };
        match first_error {
            Some(error) => Err(RegionalOwnerShutdownError {
                error,
                recovered: Box::new(recovered),
            }),
            None => Ok(recovered),
        }
    }
}
