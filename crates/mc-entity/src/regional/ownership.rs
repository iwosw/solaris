use std::collections::{BTreeMap, BTreeSet};

use super::{RegionEpoch, RegionKey, RegionLease, RegionPhase};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionOwnershipError {
    AlreadyOwned,
    UnknownRegion,
    StaleEpoch,
    EpochExhausted,
    PhaseExhausted,
    PhaseActive,
    PhaseInactive,
    StalePhase,
    UnknownLane,
    DuplicateLaneCompletion,
    LaneCompleted,
    PhaseIncomplete,
}

#[derive(Debug, Default)]
pub struct RegionOwnership {
    pub(super) owners: BTreeMap<RegionKey, RegionLease>,
    pub(super) active_phase: Option<RegionPhase>,
    pub(super) pending_lanes: BTreeSet<usize>,
    pub(super) last_phase: u64,
}

impl RegionOwnership {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn assign(
        &mut self,
        key: RegionKey,
        lane: usize,
    ) -> Result<RegionLease, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if self.owners.contains_key(&key) {
            return Err(RegionOwnershipError::AlreadyOwned);
        }
        let lease = RegionLease {
            key,
            epoch: RegionEpoch::INITIAL,
            lane,
        };
        self.owners.insert(key, lease);
        Ok(lease)
    }

    pub fn reassign(
        &mut self,
        expected: RegionLease,
        lane: usize,
    ) -> Result<RegionLease, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        let current = self
            .owners
            .get(&expected.key)
            .copied()
            .ok_or(RegionOwnershipError::UnknownRegion)?;
        if current != expected {
            return Err(RegionOwnershipError::StaleEpoch);
        }
        let epoch = current
            .epoch
            .next()
            .ok_or(RegionOwnershipError::EpochExhausted)?;
        let lease = RegionLease {
            key: current.key,
            epoch,
            lane,
        };
        self.owners.insert(current.key, lease);
        Ok(lease)
    }

    pub(super) fn unassign(&mut self, expected: RegionLease) -> Result<(), RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if self.owners.get(&expected.key).copied() != Some(expected) {
            return Err(RegionOwnershipError::StaleEpoch);
        }
        self.owners.remove(&expected.key);
        Ok(())
    }

    #[must_use]
    pub fn validate(&self, lease: RegionLease) -> bool {
        self.owners
            .get(&lease.key)
            .is_some_and(|current| *current == lease)
    }

    #[must_use]
    pub fn lease(&self, key: RegionKey) -> Option<RegionLease> {
        self.owners.get(&key).copied()
    }

    pub fn leases(&self) -> impl ExactSizeIterator<Item = RegionLease> + '_ {
        self.owners.values().copied()
    }

    pub fn begin_phase(&mut self) -> Result<RegionPhase, RegionOwnershipError> {
        let lanes = self.owners.values().map(|lease| lease.lane).collect();
        self.begin_phase_for_lanes(lanes)
    }

    pub(super) fn begin_phase_for_lanes(
        &mut self,
        lanes: BTreeSet<usize>,
    ) -> Result<RegionPhase, RegionOwnershipError> {
        let next = self
            .last_phase
            .checked_add(1)
            .ok_or(RegionOwnershipError::PhaseExhausted)?;
        self.begin_allocated_phase_for_lanes(RegionPhase(next), lanes)
    }

    pub(super) fn begin_allocated_phase_for_lanes(
        &mut self,
        phase: RegionPhase,
        lanes: BTreeSet<usize>,
    ) -> Result<RegionPhase, RegionOwnershipError> {
        if self.active_phase.is_some() {
            return Err(RegionOwnershipError::PhaseActive);
        }
        if lanes
            .iter()
            .any(|lane| !self.owners.values().any(|lease| lease.lane == *lane))
        {
            return Err(RegionOwnershipError::UnknownLane);
        }
        if phase.0 <= self.last_phase {
            return Err(RegionOwnershipError::PhaseExhausted);
        }
        self.last_phase = phase.0;
        self.active_phase = Some(phase);
        self.pending_lanes = lanes;
        Ok(phase)
    }

    pub fn acknowledge_lane(
        &mut self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if self.pending_lanes.remove(&lane) {
            return Ok(());
        }
        if self.owners.values().any(|lease| lease.lane == lane) {
            Err(RegionOwnershipError::DuplicateLaneCompletion)
        } else {
            Err(RegionOwnershipError::UnknownLane)
        }
    }

    pub fn finish_phase(&mut self, expected: RegionPhase) -> Result<(), RegionOwnershipError> {
        self.validate_finish(expected)?;
        self.active_phase = None;
        Ok(())
    }

    pub(super) fn validate_finish(
        &self,
        expected: RegionPhase,
    ) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if !self.pending_lanes.is_empty() {
            return Err(RegionOwnershipError::PhaseIncomplete);
        }
        Ok(())
    }

    pub(super) fn validate_lane(
        &self,
        expected: RegionPhase,
        lane: usize,
    ) -> Result<(), RegionOwnershipError> {
        let Some(active) = self.active_phase else {
            return Err(RegionOwnershipError::PhaseInactive);
        };
        if active != expected {
            return Err(RegionOwnershipError::StalePhase);
        }
        if self.pending_lanes.contains(&lane) {
            return Ok(());
        }
        if self.owners.values().any(|lease| lease.lane == lane) {
            Err(RegionOwnershipError::LaneCompleted)
        } else {
            Err(RegionOwnershipError::UnknownLane)
        }
    }

    #[must_use]
    pub fn validate_phase(&self, phase: RegionPhase) -> bool {
        self.active_phase == Some(phase)
    }
}
