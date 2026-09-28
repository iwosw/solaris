use super::*;

impl RegionalEntityStore {
    #[must_use]
    pub fn new() -> Self {
        Self {
            authority: next_regional_authority_id(),
            ownership: RegionOwnership::new(),
            stores: BTreeMap::new(),
            locations: BTreeMap::new(),
            uuids: HashMap::new(),
            transfers: BTreeMap::new(),
            in_flight_transfers: BTreeMap::new(),

            next_id: 0,
        }
    }

    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        let mut store = Self::new();
        store.next_id = next_id.max(0);
        store
    }

    pub fn assign_region(
        &mut self,
        key: RegionKey,
        lane: usize,
    ) -> Result<RegionLease, RegionEntityStoreError> {
        let lease = self.ownership.assign(key, lane)?;
        self.stores.insert(key, EntityStore::new());
        Ok(lease)
    }

    pub fn reassign_region(
        &mut self,
        expected: RegionLease,
        lane: usize,
    ) -> Result<RegionLease, RegionEntityStoreError> {
        Ok(self.ownership.reassign(expected, lane)?)
    }

    pub fn begin_phase(&mut self) -> Result<RegionPhase, RegionEntityStoreError> {
        Ok(self.ownership.begin_phase()?)
    }

    pub fn finish_phase(&mut self, expected: RegionPhase) -> Result<(), RegionEntityStoreError> {
        self.ownership.validate_finish(expected)?;
        let transfers = self
            .transfers
            .iter()
            .filter_map(|(&id, transfer)| {
                (transfer.phase == expected && transfer.applied.is_none()).then_some(id)
            })
            .collect::<Vec<_>>();
        for transfer in transfers {
            if self
                .transfers
                .get(&transfer)
                .is_some_and(|prepared| prepared.decision.is_none())
            {
                self.decide_transfer(expected, transfer, TransferDecision::Reject)?;
            }
            self.apply_transfer(expected, transfer)?;
        }
        self.ownership.finish_phase(expected)?;
        self.transfers
            .retain(|_, transfer| transfer.phase != expected);
        debug_assert!(self.in_flight_transfers.is_empty());
        Ok(())
    }

    pub fn acknowledge_lane(
        &mut self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionEntityStoreError> {
        Ok(self.ownership.acknowledge_lane(expected, lane)?)
    }

    pub fn prepare_transfer(
        &mut self,
        phase: RegionPhase,
        source: RegionLease,
        target: RegionLease,
        tick: u64,
        state: EntityKinematics,
    ) -> Result<TransferId, RegionEntityStoreError> {
        self.validate_access(phase, source)?;
        self.validate_access(phase, target)?;
        if !state.is_finite() {
            return Err(RegionEntityStoreError::InvalidKinematics);
        }
        if source.key == target.key {
            return Err(RegionEntityStoreError::SameRegionTransfer);
        }
        if RegionKey::from_position(state.position) != Some(target.key) {
            return Err(RegionEntityStoreError::WrongTargetRegion);
        }
        let transfer = TransferId {
            tick,
            source: source.key,
            source_epoch: source.epoch,
            entity: state.id,
        };
        if let Some(existing) = self.transfers.get(&transfer) {
            return if existing.matches_request(phase, source, target, state) {
                Ok(transfer)
            } else {
                Err(RegionEntityStoreError::TransferConflict)
            };
        }
        if self.locations.get(&state.id).copied() != Some(source.key) {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }

        let source_snapshot = self
            .stores
            .get(&source.key)
            .and_then(|store| store.snapshot(state.id))
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let source_snapshots = self.vehicle_group_snapshots(source.key, state.id)?;
        if vehicle_group_leader(&source_snapshots) != Some(state.id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let group_ids = source_snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>();
        if group_ids
            .iter()
            .any(|id| self.in_flight_transfers.contains_key(id))
        {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let delta = Vec3::new(
            state.position.x - source_snapshot.position.x,
            state.position.y - source_snapshot.position.y,
            state.position.z - source_snapshot.position.z,
        );
        let mut target_snapshots = Vec::with_capacity(source_snapshots.len());
        for source_snapshot in &source_snapshots {
            let mut target_snapshot = source_snapshot.clone();
            if source_snapshot.id == state.id {
                target_snapshot.position = state.position;
                target_snapshot.rotation = state.rotation;
                target_snapshot.velocity = state.velocity;
                target_snapshot.on_ground = state.on_ground;
            } else {
                target_snapshot.position = Vec3::new(
                    source_snapshot.position.x + delta.x,
                    source_snapshot.position.y + delta.y,
                    source_snapshot.position.z + delta.z,
                );
            }
            if RegionKey::from_position(target_snapshot.position) != Some(target.key) {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            if snapshot_vehicle_reference(&target_snapshot).is_some_and(|referenced| {
                !group_ids.contains(&referenced) && self.region_for(referenced) != Some(target.key)
            }) || goal_reference(&target_snapshot.goal)
                .is_some_and(|referenced| !self.locations.contains_key(&referenced))
            {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            target_snapshots.push(target_snapshot);
        }
        if self.has_incoming_cross_region_reference(&group_ids, target.key) {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }

        let prepared = PreparedTransfer {
            phase,
            source,
            target,
            source_snapshots,
            target_snapshots,
            decision: None,
            applied: None,
        };
        let target_store = self
            .stores
            .get(&target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared.target_snapshots.iter().any(|snapshot| {
            target_store.contains(snapshot.id) || target_store.contains_uuid(snapshot.uuid)
        }) {
            return Err(RegionEntityStoreError::TargetConflict);
        }
        self.transfers.insert(transfer, prepared);
        for id in group_ids {
            self.in_flight_transfers.insert(id, transfer);
        }
        Ok(transfer)
    }

    pub fn apply_kinematics(
        &mut self,
        phase: RegionPhase,
        tick: u64,
        state: EntityKinematics,
    ) -> Result<RegionalKinematicsApply, RegionEntityStoreError> {
        if !state.is_finite() {
            return Err(RegionEntityStoreError::InvalidKinematics);
        }
        let source_key = self
            .region_for(state.id)
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let source = self
            .ownership
            .lease(source_key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        self.validate_access(phase, source)?;
        if self.in_flight_transfers.contains_key(&state.id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let target_key = RegionKey::from_position(state.position)
            .ok_or(RegionEntityStoreError::InvalidKinematics)?;
        if target_key == source_key {
            let applied = self
                .stores
                .get_mut(&source_key)
                .ok_or(RegionEntityStoreError::StaleLease)?
                .apply_kinematics([state]);
            return if applied == 1 {
                Ok(RegionalKinematicsApply::AppliedLocal)
            } else {
                Err(RegionEntityStoreError::UnknownEntity)
            };
        }
        let target = self
            .ownership
            .lease(target_key)
            .ok_or(RegionEntityStoreError::Ownership(
                RegionOwnershipError::UnknownRegion,
            ))?;
        let transfer = self.prepare_transfer(phase, source, target, tick, state)?;
        Ok(RegionalKinematicsApply::PreparedTransfer(transfer))
    }

    pub fn set_velocity(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        velocity: Vec3,
    ) -> Result<bool, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_velocity(id, velocity))
    }

    pub fn set_animal_state(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        animal: AnimalBreedingState,
    ) -> Result<bool, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_animal_state(id, animal))
    }

    pub fn set_goal(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        goal: GoalState,
    ) -> Result<bool, RegionEntityStoreError> {
        if goal_reference(&goal).is_some_and(|target| !self.locations.contains_key(&target)) {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .set_goal(id, goal))
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        phase: RegionPhase,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> Result<RegionalPreparedGoalTick, RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }

        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            if let Some(key) = self.locations.get(id).copied() {
                ids_by_region.entry(key).or_default().insert(*id);
            }
        }
        for key in ids_by_region.keys() {
            let lease = self
                .ownership
                .lease(*key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            self.validate_access(phase, lease)?;
        }

        let simulation_ids = ids_by_region
            .iter()
            .map(|(&key, ids)| {
                let mut ids = ids.iter().copied().collect::<Vec<_>>();
                ids.sort_unstable();
                (key, ids)
            })
            .collect();
        let captured = self.capture_follow_targets(&ids_by_region);

        let mut batches = BTreeMap::new();
        for (key, ids) in ids_by_region {
            let prepared = self
                .stores
                .get_mut(&key)
                .expect("regional goal stores were preflighted")
                .prepare_goal_tick_with_pathing_for_ids(tick, &ids);
            batches.insert(key, prepared);
        }
        let lease_keys = batches
            .keys()
            .copied()
            .chain(captured.sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        Ok(RegionalPreparedGoalTick {
            authority: self.authority,
            phase,
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    (
                        key,
                        self.ownership
                            .lease(key)
                            .expect("prepared regional goal lease"),
                    )
                })
                .collect(),
            lane_versions: BTreeMap::new(),
            simulation_ids,
            batches,
            follow_targets: captured.complete_by_region,
            follow_target_sources: captured.sources,
            expected_missing_follow_targets: captured.expected_missing_follow_targets,
            goal_inputs: captured.inputs,
            goal_overrides: BTreeMap::new(),
            snapshot_overrides: BTreeMap::new(),
            villager_updates: BTreeMap::new(),
            cross_region_villager_candidates: HashSet::new(),
            pathing_aabbs: HashMap::new(),
        })
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        phase: RegionPhase,
        resolved: RegionalResolvedGoalTick,
    ) -> Result<GoalTickStats, RegionEntityStoreError> {
        if resolved.authority != self.authority {
            return Err(RegionEntityStoreError::StaleLease);
        }
        if resolved.phase != phase || !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.locations.contains_key(target))
        {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        if !self.follow_target_sources_match(&resolved.follow_target_sources) {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        if !self.goal_inputs_match(&resolved.goal_inputs) {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        for (&key, &lease) in &resolved.leases {
            if lease.key != key {
                return Err(RegionEntityStoreError::StaleLease);
            }
            self.validate_access(phase, lease)?;
        }

        let mut total = GoalTickStats::default();
        let mut follow_targets = resolved.follow_targets;
        for (key, batch) in resolved.batches {
            let targets = follow_targets.remove(&key).unwrap_or_default();
            let stats = self
                .stores
                .get_mut(&key)
                .expect("regional goal stores were preflighted")
                .apply_prepared_goal_tick_with_follow_targets(batch, &targets);
            add_goal_tick_stats(&mut total, stats);
        }
        Ok(total)
    }

    pub fn damage(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
        request: EntityDamageRequest,
    ) -> Result<Option<EntityDamage>, RegionEntityStoreError> {
        Ok(self
            .store_for_entity_mutation(phase, id)?
            .damage(id, request))
    }

    pub fn decide_transfer(
        &mut self,
        phase: RegionPhase,
        transfer: TransferId,
        decision: TransferDecision,
    ) -> Result<(), RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        let prepared = self
            .transfers
            .get_mut(&transfer)
            .ok_or(RegionEntityStoreError::UnknownTransfer)?;
        if prepared.phase != phase {
            return Err(RegionEntityStoreError::StalePhase);
        }
        match prepared.decision {
            None => prepared.decision = Some(decision),
            Some(existing) if existing == decision => {}
            Some(_) => return Err(RegionEntityStoreError::DecisionConflict),
        }
        Ok(())
    }

    pub fn apply_transfer(
        &mut self,
        phase: RegionPhase,
        transfer: TransferId,
    ) -> Result<TransferApply, RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        let prepared = self
            .transfers
            .get(&transfer)
            .cloned()
            .ok_or(RegionEntityStoreError::UnknownTransfer)?;
        if prepared.phase != phase {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if let Some(applied) = prepared.applied {
            return Ok(applied);
        }
        let decision = prepared
            .decision
            .ok_or(RegionEntityStoreError::TransferUndecided)?;
        if decision == TransferDecision::Reject {
            self.finish_transfer(transfer, TransferApply::Rejected);
            return Ok(TransferApply::Rejected);
        }

        if prepared
            .source_snapshots
            .iter()
            .any(|snapshot| self.locations.get(&snapshot.id).copied() != Some(prepared.source.key))
        {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }
        let source_store = self
            .stores
            .get(&prepared.source.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared
            .source_snapshots
            .iter()
            .any(|snapshot| source_store.snapshot(snapshot.id).as_ref() != Some(snapshot))
        {
            return Err(RegionEntityStoreError::SourceChanged);
        }
        let target_store = self
            .stores
            .get(&prepared.target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if prepared.target_snapshots.iter().any(|snapshot| {
            target_store.contains(snapshot.id) || target_store.contains_uuid(snapshot.uuid)
        }) {
            return Err(RegionEntityStoreError::TargetConflict);
        }

        let source_ids = prepared
            .source_snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>();
        for &id in &source_ids {
            let removed = self
                .stores
                .get_mut(&prepared.source.key)
                .and_then(|store| store.remove(id));
            if removed.is_none() {
                return Err(RegionEntityStoreError::UnknownEntity);
            }
        }
        let inserted = self
            .stores
            .get_mut(&prepared.target.key)
            .ok_or(RegionEntityStoreError::StaleLease)?
            .insert_snapshots_batch(prepared.target_snapshots.clone());
        if !inserted {
            let restored = self
                .stores
                .get_mut(&prepared.source.key)
                .is_some_and(|store| {
                    store.insert_snapshots_batch(prepared.source_snapshots.clone())
                });
            assert!(
                restored,
                "validated transfer rollback must restore source authority"
            );
            return Err(RegionEntityStoreError::TargetConflict);
        }
        for id in source_ids {
            self.locations.insert(id, prepared.target.key);
        }
        self.finish_transfer(transfer, TransferApply::Committed);
        Ok(TransferApply::Committed)
    }

    pub fn spawn(
        &mut self,
        phase: RegionPhase,
        lease: RegionLease,
        entity: SpawnEntity,
    ) -> Result<EntityId, RegionEntityStoreError> {
        self.validate_access(phase, lease)?;
        if RegionKey::from_position(entity.position) != Some(lease.key) {
            return Err(RegionEntityStoreError::WrongSpawnRegion);
        }
        let passenger = entity_vehicle_reference(&entity);
        if passenger.is_some_and(|id| self.in_flight_transfers.contains_key(&id)) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        if passenger.is_some_and(|id| self.region_for(id) != Some(lease.key))
            || goal_reference(&entity.goal)
                .is_some_and(|target| !self.locations.contains_key(&target))
        {
            return Err(RegionEntityStoreError::CrossRegionReference);
        }
        let id = self.next_available_id()?;
        let next_id_after_reservation =
            id.0.checked_add(i32::from(entity.reserved_following_ids))
                .ok_or(RegionEntityStoreError::IdExhausted)?;
        let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
        if self.uuids.contains_key(&uuid) {
            return Err(RegionEntityStoreError::DuplicateUuid);
        }
        let snapshot = snapshot_from_spawn(id, uuid, entity);
        let store = self
            .stores
            .get_mut(&lease.key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if !store.insert_snapshot(snapshot) {
            return Err(RegionEntityStoreError::TargetConflict);
        }
        self.locations.insert(id, lease.key);
        self.uuids.insert(uuid, id);
        self.next_id = next_id_after_reservation;
        Ok(id)
    }

    pub fn spawn_batch(
        &mut self,
        phase: RegionPhase,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Result<Vec<EntityId>, RegionEntityStoreError> {
        let mut pending = Vec::new();
        let mut pending_ids = HashSet::new();
        let mut pending_uuids = HashSet::new();
        let mut cursor = self.next_id;
        for entity in entities {
            let key = RegionKey::from_position(entity.position)
                .ok_or(RegionEntityStoreError::WrongSpawnRegion)?;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionEntityStoreError::Ownership(
                    RegionOwnershipError::UnknownRegion,
                ))?;
            self.validate_access(phase, lease)?;
            let id = self.next_available_id_after(cursor, &pending_ids)?;
            let reserved_following_ids = entity.reserved_following_ids;
            let uuid = entity.uuid.unwrap_or_else(|| deterministic_uuid(id));
            if self.uuids.contains_key(&uuid) || !pending_uuids.insert(uuid) {
                return Err(RegionEntityStoreError::DuplicateUuid);
            }
            pending_ids.insert(id);
            cursor =
                id.0.checked_add(i32::from(reserved_following_ids))
                    .ok_or(RegionEntityStoreError::IdExhausted)?;
            pending.push(PendingInsert {
                key,
                snapshot: snapshot_from_spawn(id, uuid, entity),
            });
        }
        let ids = pending
            .iter()
            .map(|entity| entity.snapshot.id)
            .collect::<Vec<_>>();
        self.insert_prepared_snapshots(phase, pending)?;
        self.next_id = self.next_id.max(cursor);
        Ok(ids)
    }

    pub fn insert_snapshots(
        &mut self,
        phase: RegionPhase,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> Result<usize, RegionEntityStoreError> {
        let mut pending = Vec::new();
        let mut pending_ids = HashSet::new();
        let mut pending_uuids = HashSet::new();
        for snapshot in snapshots {
            if self.contains(snapshot.id) || !pending_ids.insert(snapshot.id) {
                return Err(RegionEntityStoreError::TargetConflict);
            }
            if self.contains_uuid(snapshot.uuid) || !pending_uuids.insert(snapshot.uuid) {
                return Err(RegionEntityStoreError::DuplicateUuid);
            }
            let key = RegionKey::from_position(snapshot.position)
                .ok_or(RegionEntityStoreError::WrongSpawnRegion)?;
            let lease = self
                .ownership
                .lease(key)
                .ok_or(RegionEntityStoreError::Ownership(
                    RegionOwnershipError::UnknownRegion,
                ))?;
            self.validate_access(phase, lease)?;
            pending.push(PendingInsert { key, snapshot });
        }
        let count = pending.len();
        self.insert_prepared_snapshots(phase, pending)?;
        Ok(count)
    }

    pub fn remove(
        &mut self,
        phase: RegionPhase,
        lease: RegionLease,
        id: EntityId,
    ) -> Result<EntitySnapshot, RegionEntityStoreError> {
        self.validate_access(phase, lease)?;
        if self.in_flight_transfers.contains_key(&id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        if self.locations.get(&id).copied() != Some(lease.key) {
            return Err(RegionEntityStoreError::WrongSourceRegion);
        }
        let removed = self
            .stores
            .get_mut(&lease.key)
            .and_then(|store| store.remove(id))
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        self.locations.remove(&id);
        self.uuids.remove(&removed.uuid);
        Ok(removed)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        let key = self.locations.get(&id)?;
        self.stores.get(key)?.snapshot(id)
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.locations.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.locations.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.locations.contains_key(&id)
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.uuids.contains_key(&uuid)
    }

    #[must_use]
    pub fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        let key = self.locations.get(&id)?;
        self.stores.get(key)?.motion_state(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        self.locations.keys().filter_map(|&id| self.snapshot(id))
    }

    pub fn visit_simulation_entities(&self, mut visitor: impl FnMut(EntityView<'_>)) {
        for &id in self.locations.keys() {
            self.visit_entity(id, &mut visitor);
        }
    }

    pub fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        let mut ordered = ids
            .iter()
            .filter(|id| self.locations.contains_key(id))
            .copied()
            .collect::<Vec<_>>();
        ordered.sort_unstable();
        for id in ordered {
            self.visit_entity(id, &mut visitor);
        }
    }

    pub fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        mut visitor: impl FnMut(EntityView<'_>),
    ) {
        let mut ordered = Vec::new();
        for store in self.stores.values() {
            store.visit_sheep_entities_for_ids(ids, |entity| ordered.push(entity.id));
        }
        ordered.sort_unstable();
        for id in ordered {
            self.visit_entity(id, &mut visitor);
        }
    }

    #[must_use]
    pub fn region_for(&self, id: EntityId) -> Option<RegionKey> {
        self.locations.get(&id).copied()
    }

    #[must_use]
    pub fn region_len(&self, key: RegionKey) -> usize {
        self.stores.get(&key).map_or(0, EntityStore::len)
    }

    fn validate_access(
        &self,
        phase: RegionPhase,
        lease: RegionLease,
    ) -> Result<(), RegionEntityStoreError> {
        if !self.ownership.validate_phase(phase) {
            return Err(RegionEntityStoreError::StalePhase);
        }
        if !self.ownership.validate(lease) || !self.stores.contains_key(&lease.key) {
            return Err(RegionEntityStoreError::StaleLease);
        }
        self.ownership.validate_lane(phase, lease.lane)?;
        Ok(())
    }

    fn has_incoming_cross_region_reference(
        &self,
        targets: &HashSet<EntityId>,
        destination: RegionKey,
    ) -> bool {
        self.locations.iter().any(|(&id, &region)| {
            !targets.contains(&id)
                && region != destination
                && self
                    .stores
                    .get(&region)
                    .and_then(|store| store.snapshot(id))
                    .and_then(|entity| snapshot_vehicle_reference(&entity))
                    .is_some_and(|referenced| targets.contains(&referenced))
        })
    }

    pub(super) fn vehicle_group_snapshots(
        &self,
        source: RegionKey,
        root: EntityId,
    ) -> Result<Vec<EntitySnapshot>, RegionEntityStoreError> {
        let store = self
            .stores
            .get(&source)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        if !store.contains(root) {
            return Err(RegionEntityStoreError::UnknownEntity);
        }
        let snapshots = store.snapshots().collect::<Vec<_>>();
        let mut group = HashSet::from([root]);
        loop {
            let before = group.len();
            for snapshot in &snapshots {
                let passenger = snapshot.vehicle.and_then(|vehicle| vehicle.passenger);
                if group.contains(&snapshot.id) {
                    if let Some(passenger) = passenger {
                        group.insert(passenger);
                    }
                } else if passenger.is_some_and(|passenger| group.contains(&passenger)) {
                    group.insert(snapshot.id);
                }
            }
            if group.len() == before {
                break;
            }
        }
        let group = snapshots
            .into_iter()
            .filter(|snapshot| group.contains(&snapshot.id))
            .collect::<Vec<_>>();
        if group.len() == 1 && group[0].id != root {
            return Err(RegionEntityStoreError::UnknownEntity);
        }
        Ok(group)
    }

    pub(super) fn capture_follow_targets(
        &self,
        ids_by_region: &BTreeMap<RegionKey, HashSet<EntityId>>,
    ) -> CapturedFollowTargets {
        let mut complete_by_region = BTreeMap::new();
        let mut sources = BTreeMap::new();
        let mut expected_missing_follow_targets = HashSet::new();
        let mut inputs = BTreeMap::new();
        for (&region, ids) in ids_by_region {
            let mut targets = HashMap::new();
            let mut region_inputs = Vec::with_capacity(ids.len());
            for &id in ids {
                let Some(follower) = self.snapshot(id) else {
                    continue;
                };
                let follower = goal_checkpoint_from_snapshot(follower);
                region_inputs.push(follower.clone());
                let Some(target) = goal_reference(&follower.goal) else {
                    continue;
                };
                let Some(target_region) = self.region_for(target) else {
                    expected_missing_follow_targets.insert(target);
                    continue;
                };
                let Some(target_snapshot) = self.snapshot(target) else {
                    expected_missing_follow_targets.insert(target);
                    continue;
                };
                let target_checkpoint = goal_checkpoint_from_snapshot(target_snapshot);
                sources.insert(
                    target,
                    RegionalFollowTargetSource {
                        region: target_region,
                        checkpoint: target_checkpoint.clone(),
                    },
                );
                targets.insert(target, target_checkpoint.position);
            }
            inputs.insert(region, region_inputs);
            if !targets.is_empty() {
                complete_by_region.insert(region, targets);
            }
        }
        CapturedFollowTargets {
            complete_by_region,
            sources,
            expected_missing_follow_targets,
            inputs,
        }
    }

    pub(super) fn follow_target_sources_match(
        &self,
        expected: &BTreeMap<EntityId, RegionalFollowTargetSource>,
    ) -> bool {
        expected.iter().all(|(&id, expected)| {
            self.region_for(id) == Some(expected.region)
                && self
                    .snapshot(id)
                    .map(goal_checkpoint_from_snapshot)
                    .as_ref()
                    == Some(&expected.checkpoint)
        })
    }

    pub(super) fn goal_inputs_match(
        &self,
        expected: &BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
    ) -> bool {
        expected.iter().all(|(&region, checkpoints)| {
            checkpoints.iter().all(|expected| {
                RegionKey::from_position(expected.position) == Some(region)
                    && self
                        .snapshot(expected.id)
                        .map(goal_checkpoint_from_snapshot)
                        .as_ref()
                        == Some(expected)
            })
        })
    }

    pub(super) fn store_for_entity_mutation(
        &mut self,
        phase: RegionPhase,
        id: EntityId,
    ) -> Result<&mut EntityStore, RegionEntityStoreError> {
        if self.in_flight_transfers.contains_key(&id) {
            return Err(RegionEntityStoreError::TransferConflict);
        }
        let key = self
            .region_for(id)
            .ok_or(RegionEntityStoreError::UnknownEntity)?;
        let lease = self
            .ownership
            .lease(key)
            .ok_or(RegionEntityStoreError::StaleLease)?;
        self.validate_access(phase, lease)?;
        self.stores
            .get_mut(&key)
            .ok_or(RegionEntityStoreError::StaleLease)
    }

    fn next_available_id(&self) -> Result<EntityId, RegionEntityStoreError> {
        self.next_available_id_after(self.next_id, &HashSet::new())
    }

    fn next_available_id_after(
        &self,
        cursor: i32,
        pending: &HashSet<EntityId>,
    ) -> Result<EntityId, RegionEntityStoreError> {
        let mut next_id = cursor.max(0);
        loop {
            next_id = next_id
                .checked_add(1)
                .ok_or(RegionEntityStoreError::IdExhausted)?;
            let id = EntityId(next_id);
            if !self.locations.contains_key(&id) && !pending.contains(&id) {
                return Ok(id);
            }
        }
    }

    fn insert_prepared_snapshots(
        &mut self,
        phase: RegionPhase,
        pending: Vec<PendingInsert>,
    ) -> Result<(), RegionEntityStoreError> {
        let pending_locations = pending
            .iter()
            .map(|entity| (entity.snapshot.id, entity.key))
            .collect::<HashMap<_, _>>();
        for entity in &pending {
            let lease = self
                .ownership
                .lease(entity.key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            self.validate_access(phase, lease)?;
            let passenger = snapshot_vehicle_reference(&entity.snapshot);
            if passenger.is_some_and(|id| self.in_flight_transfers.contains_key(&id)) {
                return Err(RegionEntityStoreError::TransferConflict);
            }
            if passenger.is_some_and(|id| {
                self.region_for(id)
                    .or_else(|| pending_locations.get(&id).copied())
                    != Some(entity.key)
            }) || goal_reference(&entity.snapshot.goal).is_some_and(|target| {
                !self.locations.contains_key(&target) && !pending_locations.contains_key(&target)
            }) {
                return Err(RegionEntityStoreError::CrossRegionReference);
            }
            let store = self
                .stores
                .get(&entity.key)
                .ok_or(RegionEntityStoreError::StaleLease)?;
            if store.contains(entity.snapshot.id) || store.contains_uuid(entity.snapshot.uuid) {
                return Err(RegionEntityStoreError::TargetConflict);
            }
        }

        let mut grouped = BTreeMap::<RegionKey, Vec<EntitySnapshot>>::new();
        for entity in &pending {
            grouped
                .entry(entity.key)
                .or_default()
                .push(entity.snapshot.clone());
        }
        let mut inserted = Vec::with_capacity(pending.len());
        for (key, snapshots) in grouped {
            let ids = snapshots
                .iter()
                .map(|snapshot| snapshot.id)
                .collect::<Vec<_>>();
            let success = self
                .stores
                .get_mut(&key)
                .ok_or(RegionEntityStoreError::StaleLease)?
                .insert_snapshots_batch(snapshots);
            if !success {
                for &(key, id) in &inserted {
                    let _ = self.stores.get_mut(&key).and_then(|store| store.remove(id));
                }
                return Err(RegionEntityStoreError::TargetConflict);
            }
            inserted.extend(ids.into_iter().map(|id| (key, id)));
        }
        for entity in pending {
            self.next_id = self.next_id.max(entity.snapshot.id.0);
            self.locations.insert(entity.snapshot.id, entity.key);
            self.uuids.insert(entity.snapshot.uuid, entity.snapshot.id);
        }
        Ok(())
    }

    fn visit_entity(&self, id: EntityId, visitor: &mut impl FnMut(EntityView<'_>)) {
        let Some(key) = self.locations.get(&id) else {
            return;
        };
        let Some(store) = self.stores.get(key) else {
            return;
        };
        store.visit_simulation_entities_for_ids(&HashSet::from([id]), visitor);
    }

    fn finish_transfer(&mut self, transfer: TransferId, applied: TransferApply) {
        let entity_ids = self
            .transfers
            .get_mut(&transfer)
            .map(|prepared| {
                prepared.applied = Some(applied);
                prepared
                    .source_snapshots
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect::<Vec<_>>()
            })
            .expect("known transfer");
        for id in entity_ids {
            self.in_flight_transfers.remove(&id);
        }
    }

    pub(super) fn transfer_entity_ids(&self, transfer: TransferId) -> Vec<EntityId> {
        self.transfers
            .get(&transfer)
            .map(|prepared| {
                prepared
                    .source_snapshots
                    .iter()
                    .map(|snapshot| snapshot.id)
                    .collect()
            })
            .unwrap_or_default()
    }
}
impl Default for RegionalEntityStore {
    fn default() -> Self {
        Self::new()
    }
}
