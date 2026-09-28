use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, channel, sync_channel};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::lock_policy::{
    lock_authoritative_mutex, read_authoritative_rwlock, read_benign_rwlock,
    write_authoritative_rwlock, write_benign_rwlock,
};
use crate::{
    AnimalBreedingState, EntityDamage, EntityDamageRequest, EntityEffectRequest,
    EntityEffectResult, EntityGoalCheckpoint, EntityId, EntityItemStack, EntityKinematics,
    EntityLifecycle, EntityMotionState, EntityPhysicsQuery, EntityPhysicsStep,
    EntitySimulationProjection, EntitySimulationResult, EntitySnapshot, EntityStore,
    EntityTrackingMotion, EntityView, EntityVillagerGoalUpdate, GoalState, GoalTickStats,
    PathingBudget, PathingProbe, PreparedGoalTick, ResolvedGoalTick, SpawnEntity, Vec3,
    deterministic_uuid, snapshot_from_spawn,
};

mod coordinator;
mod entity_projections;
mod handle;
mod owner_lane;
#[cfg(test)]
mod owner_lane_tests;
pub(crate) mod ownership;
#[cfg(test)]
mod ownership_tests;
mod store;
mod tick;

use owner_lane::{
    FencedKinematicsCandidate, LocalKinematicsRegionBatch, LocalPhysicsInput,
    LocalPhysicsRegionBatch, RegionalOwnerLaneReader,
};
pub use owner_lane::{
    RegionOwnerBatch, RegionOwnerCompletion, RegionOwnerLaneError, RegionOwnerLaneStartError,
    RegionOwnerMutation, RegionalOwnerLane, SequencedRegionMutation,
};
use ownership::{RegionOwnership, RegionOwnershipError};
pub use tick::{RegionalTickWorld, warm_physics_caches};

pub const REGION_SIZE_CHUNKS: i32 = 8;
const CHUNK_SIZE_BLOCKS: f64 = 16.0;
const PARALLEL_KINEMATICS_MIN_STATES: usize = 257;
const DIRECT_SELECTED_READ_LIMIT: usize = 16;
static NEXT_REGIONAL_AUTHORITY_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegionalAuthorityId(u64);

fn next_regional_authority_id() -> RegionalAuthorityId {
    let id = NEXT_REGIONAL_AUTHORITY_ID
        .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
        .expect("regional authority id space exhausted");
    RegionalAuthorityId(id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionKey {
    pub x: i32,
    pub z: i32,
}

impl RegionKey {
    #[must_use]
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    #[must_use]
    pub const fn from_chunk(chunk_x: i32, chunk_z: i32) -> Self {
        Self {
            x: chunk_x.div_euclid(REGION_SIZE_CHUNKS),
            z: chunk_z.div_euclid(REGION_SIZE_CHUNKS),
        }
    }

    #[must_use]
    pub fn from_position(position: Vec3) -> Option<Self> {
        if !position.is_finite() {
            return None;
        }
        let chunk_x = (position.x / CHUNK_SIZE_BLOCKS).floor();
        let chunk_z = (position.z / CHUNK_SIZE_BLOCKS).floor();
        if chunk_x < f64::from(i32::MIN)
            || chunk_x > f64::from(i32::MAX)
            || chunk_z < f64::from(i32::MIN)
            || chunk_z > f64::from(i32::MAX)
        {
            return None;
        }
        Some(Self::from_chunk(chunk_x as i32, chunk_z as i32))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct RegionEpoch(pub u64);

impl RegionEpoch {
    pub const INITIAL: Self = Self(1);

    #[must_use]
    pub fn next(self) -> Option<Self> {
        self.0.checked_add(1).map(Self)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionLease {
    pub key: RegionKey,
    pub epoch: RegionEpoch,
    pub lane: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct RegionPhase(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionalDecisionJournalError {
    outcome_unknown: bool,
}

impl RegionalDecisionJournalError {
    pub const SAFE: Self = Self {
        outcome_unknown: false,
    };
    pub const OUTCOME_UNKNOWN: Self = Self {
        outcome_unknown: true,
    };

    #[must_use]
    pub const fn outcome_unknown(self) -> bool {
        self.outcome_unknown
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionalCommitDecision {
    phase: RegionPhase,
    sequence_watermark: u64,
    lifecycle_epoch: u64,
    upserts: Vec<EntitySnapshot>,
    removed: Vec<EntityId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerInventoryPickupCommit {
    pub villager: (EntitySnapshot, EntitySnapshot),
    pub item: (EntitySnapshot, Option<EntitySnapshot>),
    pub item_max_stack_size: i32,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerFoodShareCommit {
    pub donor: (EntitySnapshot, EntitySnapshot),
    pub recipient: EntitySnapshot,
    pub thrown_item: SpawnEntity,
    pub food_items: crate::villager_population_26_1_2::VillagerFoodItemIds,
    pub item_max_stack_size: i32,
    pub current_tick: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerCourtshipCommit {
    pub parents: [(EntitySnapshot, EntitySnapshot); 2],
    pub current_tick: u64,
    pub food_items: crate::villager_population_26_1_2::VillagerFoodItemIds,
    pub deterministic_seed: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerNoBedCommit {
    pub parents: [(EntitySnapshot, EntitySnapshot); 2],
    pub current_tick: u64,
    pub food_items: crate::villager_population_26_1_2::VillagerFoodItemIds,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VillagerBirthCommit {
    pub parents: [(EntitySnapshot, EntitySnapshot); 2],
    pub child: SpawnEntity,
    pub current_tick: u64,
    pub food_items: crate::villager_population_26_1_2::VillagerFoodItemIds,
}

impl RegionalCommitDecision {
    pub fn from_parts(
        phase: RegionPhase,
        sequence_watermark: u64,
        upserts: Vec<EntitySnapshot>,
        removed: Vec<EntityId>,
    ) -> Result<Self, RegionalDecisionJournalError> {
        Self::from_parts_at_lifecycle_epoch(
            phase,
            sequence_watermark,
            sequence_watermark,
            upserts,
            removed,
        )
    }

    pub fn from_parts_at_lifecycle_epoch(
        phase: RegionPhase,
        sequence_watermark: u64,
        lifecycle_epoch: u64,
        mut upserts: Vec<EntitySnapshot>,
        mut removed: Vec<EntityId>,
    ) -> Result<Self, RegionalDecisionJournalError> {
        upserts.sort_unstable_by_key(|snapshot| snapshot.id);
        removed.sort_unstable();
        if upserts.windows(2).any(|pair| pair[0].id == pair[1].id)
            || removed.windows(2).any(|pair| pair[0] == pair[1])
            || upserts
                .iter()
                .any(|snapshot| removed.binary_search(&snapshot.id).is_ok())
        {
            return Err(RegionalDecisionJournalError::SAFE);
        }
        Ok(Self {
            phase,
            sequence_watermark,
            lifecycle_epoch,
            upserts,
            removed,
        })
    }
    #[must_use]
    pub const fn phase(&self) -> RegionPhase {
        self.phase
    }

    #[must_use]
    pub const fn sequence_watermark(&self) -> u64 {
        self.sequence_watermark
    }

    #[must_use]
    pub const fn lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch
    }

    #[must_use]
    pub const fn identity(&self) -> (RegionPhase, u64, u64) {
        (self.phase, self.sequence_watermark, self.lifecycle_epoch)
    }

    #[must_use]
    pub fn upserts(&self) -> &[EntitySnapshot] {
        &self.upserts
    }

    #[must_use]
    pub fn removed(&self) -> &[EntityId] {
        &self.removed
    }
}

pub trait RegionalDecisionJournal: Send {
    fn enabled(&self) -> bool {
        true
    }

    fn record_commit(
        &mut self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError>;

    fn record_commits(
        &mut self,
        decisions: &[RegionalCommitDecision],
    ) -> Result<(), RegionalDecisionJournalError> {
        for decision in decisions {
            self.record_commit(decision)?;
        }
        Ok(())
    }

    fn clear_commit(&mut self, phase: RegionPhase) -> Result<(), RegionalDecisionJournalError>;

    fn clear_commits(
        &mut self,
        phases: &[RegionPhase],
    ) -> Result<(), RegionalDecisionJournalError> {
        for phase in phases {
            self.clear_commit(*phase)?;
        }
        Ok(())
    }

    fn clear_commit_identities(
        &mut self,
        identities: &[(RegionPhase, u64, u64)],
    ) -> Result<(), RegionalDecisionJournalError> {
        for identity in identities {
            self.clear_commit(identity.0)?;
        }
        Ok(())
    }

    fn pending_phases(&self) -> Vec<RegionPhase> {
        Vec::new()
    }

    fn pending_commit_identities(&self) -> Vec<(RegionPhase, u64, u64)> {
        self.pending_phases()
            .into_iter()
            .map(|phase| (phase, 0, 0))
            .collect()
    }

    fn recovery_watermark(&self) -> (RegionPhase, u64) {
        self.pending_commit_identities()
            .into_iter()
            .fold((RegionPhase(0), 0), |(phase, sequence), identity| {
                (phase.max(identity.0), sequence.max(identity.1))
            })
    }
}

struct NoopRegionalDecisionJournal;

impl RegionalDecisionJournal for NoopRegionalDecisionJournal {
    fn enabled(&self) -> bool {
        false
    }

    fn record_commit(
        &mut self,
        _decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        Ok(())
    }

    fn clear_commit(&mut self, _phase: RegionPhase) -> Result<(), RegionalDecisionJournalError> {
        Ok(())
    }
}

#[derive(Debug)]
pub struct RegionalOwnerCutoverError {
    pub error: RegionOwnerLaneError,
    pub store: Box<RegionalEntityStore>,
}

#[derive(Debug)]
pub struct RegionalOwnerShutdownError {
    pub error: RegionOwnerLaneError,
    pub recovered: Box<RegionalEntityStore>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionalOwnerSaveSnapshot {
    sequence_watermark: u64,
    lifecycle_epoch: u64,
    snapshots: Vec<EntitySnapshot>,
    journal_phases: Vec<RegionPhase>,
}

impl RegionalOwnerSaveSnapshot {
    #[must_use]
    pub const fn sequence_watermark(&self) -> u64 {
        self.sequence_watermark
    }

    #[must_use]
    pub const fn lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch
    }

    #[must_use]
    pub fn snapshots(&self) -> &[EntitySnapshot] {
        &self.snapshots
    }

    #[must_use]
    pub fn journal_phases(&self) -> &[RegionPhase] {
        &self.journal_phases
    }

    #[must_use]
    pub fn into_snapshots(self) -> Vec<EntitySnapshot> {
        self.snapshots
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionEntityStoreError {
    Ownership(RegionOwnershipError),
    StalePhase,
    StaleLease,
    WrongSpawnRegion,
    WrongSourceRegion,
    UnknownEntity,
    DuplicateUuid,
    IdExhausted,
    CrossRegionReference,
    TargetConflict,
    SameRegionTransfer,
    WrongTargetRegion,
    InvalidKinematics,
    TransferConflict,
    UnknownTransfer,
    DecisionConflict,
    TransferUndecided,
    SourceChanged,
}

impl From<RegionOwnershipError> for RegionEntityStoreError {
    fn from(error: RegionOwnershipError) -> Self {
        Self::Ownership(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TransferId {
    pub tick: u64,
    pub source: RegionKey,
    pub source_epoch: RegionEpoch,
    pub entity: EntityId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferDecision {
    Commit,
    Reject,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransferApply {
    Committed,
    Rejected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RegionalKinematicsApply {
    AppliedLocal,
    PreparedTransfer(TransferId),
}

#[derive(Debug, Clone, PartialEq)]
struct PreparedTransfer {
    phase: RegionPhase,
    source: RegionLease,
    target: RegionLease,
    source_snapshots: Vec<EntitySnapshot>,
    target_snapshots: Vec<EntitySnapshot>,
    decision: Option<TransferDecision>,
    applied: Option<TransferApply>,
}

#[derive(Debug)]
struct PendingInsert {
    key: RegionKey,
    snapshot: EntitySnapshot,
}

#[derive(Debug, Clone)]
pub struct RegionalVillagerProfessionOffer {
    pub profession: crate::VillagerProfession,
    pub merchant: crate::villager_merchant_26_1_2::VillagerMerchantState,
}

#[derive(Debug, Clone)]
pub struct RegionalVillagerGoalTickInputs {
    pub day_time: i64,
    pub profile: Arc<crate::villager_26_1_2::VillagerBrainProfile>,
    pub profession_offers: Arc<HashMap<EntityId, RegionalVillagerProfessionOffer>>,
}

#[derive(Debug, Clone)]
pub struct RegionalGoalTickInputs {
    pub active_chunks: Option<Arc<HashSet<(i32, i32)>>>,
    pub terrain_pathing_entities: Arc<HashSet<EntityId>>,
    pub hostile_target_positions: Option<Arc<[Vec3]>>,
    pub combat_targets: Arc<HashMap<i32, Vec3>>,
    pub mob_behaviors: Arc<mc_data::mob_behavior_26_1_2::MobBehaviorTable>,
    pub villager: Option<Arc<RegionalVillagerGoalTickInputs>>,
}

impl Default for RegionalGoalTickInputs {
    fn default() -> Self {
        Self {
            active_chunks: None,
            terrain_pathing_entities: Arc::new(HashSet::new()),
            hostile_target_positions: None,
            combat_targets: Arc::new(HashMap::new()),
            mob_behaviors: Arc::new(
                mc_data::mob_behavior_26_1_2::MobBehaviorTable::vanilla_26_1_2(),
            ),
            villager: None,
        }
    }
}

#[derive(Clone)]
pub struct RegionalEntityTickInput {
    pub tick: u64,
    pub simulation_chunks: Arc<HashSet<(i32, i32)>>,
    pub goals: RegionalGoalTickInputs,
    pub world: Arc<RegionalTickWorld>,
    pub pathing_budget: PathingBudget,
    pub profession_offers_by_block_state: Arc<HashMap<u32, RegionalVillagerProfessionOffer>>,
}

pub struct RegionalEntityTickOutput {
    pub goal_stats: GoalTickStats,
    pub active_entity_count: usize,
    pub active_hostile_ids: Vec<(EntityId, Vec3)>,
    pub villager_population_candidates: Vec<(EntityId, Vec3)>,
    pub villager_ids: Vec<(EntityId, Vec3)>,
    pub villager_proximity_seeds: Vec<(EntityId, Vec3)>,
    pub fallback_entity_ids: HashSet<EntityId>,
    pub prepared_physics: RegionalPreparedEntityPhysics,
    pub resolved_direct_paths: HashSet<EntityId>,
    pub villager_profession_updates: Vec<EntitySnapshot>,
}

pub struct RegionalPreparedEntityPhysics {
    authority: RegionalAuthorityId,
    simulation_chunks: Arc<HashSet<(i32, i32)>>,
    world: Arc<RegionalTickWorld>,
    lanes: Vec<RegionalPreparedLanePhysics>,
}

struct RegionalPreparedLanePhysics {
    lane: usize,
    regions: Vec<RegionLease>,
    goal_committed_motion: Vec<(RegionLease, EntityTrackingMotion)>,
}

/// Lane-side timing split for one owned-region physics commit. All fields are
/// wall time in microseconds on the lane/coordinator clock. `worker_wait_us`
/// is the summed sequential nested per-lane completion wait (aggregate stall
/// on the coordinator); `worker_exec_us` is the slowest single per-lane
/// worker execution (max). With one lane, worker message-queue wait is
/// approximately `worker_wait_us` minus `worker_exec_us`; with several lanes
/// compare max-to-max instead of subtracting aggregates.
#[derive(Debug, Clone, Copy, Default)]
pub struct LaneCommitTimings {
    /// Whole coordinator execution: admission locks, lane dispatch, nested
    /// worker completion, validation and fence assembly.
    pub exec_us: u64,
    /// Waiting to acquire all lane admission locks.
    pub admission_wait_us: u64,
    /// Waiting on nested per-lane worker completion channels.
    pub worker_wait_us: u64,
    /// Slowest per-lane worker execution of this physics message itself
    /// (dispatch receipt to output assembly on the worker thread).
    pub worker_exec_us: u64,
}

pub struct RegionalEntityPhysicsOutput {
    pub fallback_results: Vec<EntitySimulationResult>,
    pub fallback_fence: Option<VersionedEntityKinematics>,
    pub committed_motion: Vec<EntityTrackingMotion>,
    pub publication_fence: VersionedEntitySnapshots,
    pub terrain_pathing_additions: HashSet<EntityId>,
    pub lane_timings: LaneCommitTimings,
}
#[derive(Debug)]
pub struct RegionalPreparedGoalTick {
    authority: RegionalAuthorityId,
    phase: RegionPhase,
    leases: BTreeMap<RegionKey, RegionLease>,
    lane_versions: BTreeMap<usize, u64>,
    batches: BTreeMap<RegionKey, PreparedGoalTick>,
    simulation_ids: BTreeMap<RegionKey, Vec<EntityId>>,
    follow_targets: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    follow_target_sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,
    goal_inputs: BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
    goal_overrides: BTreeMap<RegionKey, Vec<(EntityId, GoalState)>>,
    snapshot_overrides: BTreeMap<RegionKey, Vec<(EntitySnapshot, EntitySnapshot)>>,
    villager_updates: BTreeMap<RegionKey, Vec<EntityVillagerGoalUpdate>>,
    cross_region_villager_candidates: HashSet<EntityId>,
    pathing_aabbs: HashMap<EntityId, mc_physics::Aabb>,
}

#[derive(Debug)]
pub struct RegionalResolvedGoalTick {
    authority: RegionalAuthorityId,
    phase: RegionPhase,
    leases: BTreeMap<RegionKey, RegionLease>,
    lane_versions: BTreeMap<usize, u64>,
    batches: BTreeMap<RegionKey, ResolvedGoalTick>,
    simulation_ids: BTreeMap<RegionKey, Vec<EntityId>>,
    follow_targets: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    follow_target_sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,
    goal_inputs: BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
    goal_overrides: BTreeMap<RegionKey, Vec<(EntityId, GoalState)>>,
    snapshot_overrides: BTreeMap<RegionKey, Vec<(EntitySnapshot, EntitySnapshot)>>,
    villager_updates: BTreeMap<RegionKey, Vec<EntityVillagerGoalUpdate>>,
}

#[derive(Debug, Clone, PartialEq)]
struct RegionalFollowTargetSource {
    region: RegionKey,
    checkpoint: EntityGoalCheckpoint,
}

struct CapturedFollowTargets {
    complete_by_region: BTreeMap<RegionKey, HashMap<EntityId, Vec3>>,
    sources: BTreeMap<EntityId, RegionalFollowTargetSource>,
    expected_missing_follow_targets: HashSet<EntityId>,

    inputs: BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
}

impl RegionalPreparedGoalTick {
    #[must_use]
    pub fn parallel_batch_count(&self) -> usize {
        self.batches
            .values()
            .filter(|batch| batch.pathing_request_count() > 0)
            .count()
    }
    #[must_use]
    pub fn pathing_request_count(&self) -> usize {
        self.batches
            .values()
            .map(PreparedGoalTick::pathing_request_count)
            .sum()
    }

    pub fn goal_entity_count(&self) -> usize {
        self.goal_inputs.values().map(Vec::len).sum::<usize>() + self.pathing_request_count()
    }

    #[must_use]
    pub fn pathing_aabbs(&self) -> &HashMap<EntityId, mc_physics::Aabb> {
        &self.pathing_aabbs
    }

    #[must_use]
    pub fn cross_region_villager_candidates(&self) -> &HashSet<EntityId> {
        &self.cross_region_villager_candidates
    }

    pub fn visit_pathing_probe_positions(
        &self,
        budget: PathingBudget,
        mut visitor: impl FnMut(EntityId, Vec3),
    ) {
        for batch in self.batches.values() {
            batch.visit_pathing_probe_positions(budget, &mut visitor);
        }
    }

    #[must_use]
    pub fn resolve(
        self,
        probe: &dyn PathingProbe,
        budget: PathingBudget,
    ) -> RegionalResolvedGoalTick {
        let mut goal_inputs = self.goal_inputs;
        let mut batches = BTreeMap::new();
        for (key, prepared) in self.batches {
            remove_pathing_checkpoints(&mut goal_inputs, key, &prepared);
            let (resolved, checkpoints) = prepared.resolve_for_regional(probe, budget);
            insert_resolved_goal_batch(&mut goal_inputs, &mut batches, key, resolved, checkpoints);
        }
        RegionalResolvedGoalTick {
            authority: self.authority,
            phase: self.phase,
            leases: self.leases,
            lane_versions: self.lane_versions,
            simulation_ids: self.simulation_ids,
            follow_targets: self.follow_targets,
            follow_target_sources: self.follow_target_sources,
            expected_missing_follow_targets: self.expected_missing_follow_targets,
            goal_inputs,
            goal_overrides: self.goal_overrides,
            snapshot_overrides: self.snapshot_overrides,
            villager_updates: self.villager_updates,
            batches,
        }
    }

    #[must_use]
    pub fn resolve_parallel(
        self,
        probe: &(dyn PathingProbe + Sync),
        budget: PathingBudget,
        max_workers: usize,
    ) -> RegionalResolvedGoalTick {
        let worker_count = max_workers.max(1).min(self.parallel_batch_count().max(1));
        if worker_count == 1 {
            return self.resolve(probe, budget);
        }

        let Self {
            authority,
            phase,
            leases,
            lane_versions,
            simulation_ids,
            batches,
            follow_targets,
            follow_target_sources,
            expected_missing_follow_targets,
            mut goal_inputs,
            goal_overrides,
            snapshot_overrides,
            villager_updates,
            cross_region_villager_candidates: _,
            pathing_aabbs: _,
        } = self;
        for (&key, prepared) in &batches {
            remove_pathing_checkpoints(&mut goal_inputs, key, prepared);
        }
        let batch_count = batches.len();
        let (parallel_batches, inline_batches): (Vec<_>, Vec<_>) = batches
            .into_iter()
            .partition(|(_, batch)| batch.pathing_request_count() > 0);
        let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (index, batch) in parallel_batches.into_iter().enumerate() {
            buckets[index % worker_count].push(batch);
        }
        let local = buckets.pop().expect("positive regional worker count");
        let resolved = std::sync::Mutex::new(Vec::with_capacity(batch_count));
        rayon::scope(|scope| {
            for bucket in buckets {
                let resolved = &resolved;
                scope.spawn(move |_| {
                    let worker_results = bucket
                        .into_iter()
                        .map(|(key, prepared)| {
                            let (resolved, checkpoints) =
                                prepared.resolve_for_regional(probe, budget);
                            (key, resolved, checkpoints)
                        })
                        .collect::<Vec<_>>();
                    lock_authoritative_mutex(resolved, "regional.goal_parallel_results")
                        .extend(worker_results);
                });
            }
            let local_results = inline_batches
                .into_iter()
                .chain(local)
                .map(|(key, prepared)| {
                    let (resolved, checkpoints) = prepared.resolve_for_regional(probe, budget);
                    (key, resolved, checkpoints)
                })
                .collect::<Vec<_>>();
            lock_authoritative_mutex(&resolved, "regional.goal_parallel_results")
                .extend(local_results);
        });
        let resolved = resolved
            .into_inner()
            .expect("regional goal parallel result lock poisoned");
        let mut batches = BTreeMap::new();
        for (key, resolved, checkpoints) in resolved {
            insert_resolved_goal_batch(&mut goal_inputs, &mut batches, key, resolved, checkpoints);
        }

        RegionalResolvedGoalTick {
            authority,
            phase,
            leases,
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
        }
    }
}

fn remove_pathing_checkpoints(
    goal_inputs: &mut BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
    region: RegionKey,
    prepared: &PreparedGoalTick,
) {
    let Some(inputs) = goal_inputs.get_mut(&region) else {
        return;
    };
    let pathing_ids = prepared.pathing_request_ids().collect::<HashSet<_>>();
    if !pathing_ids.is_empty() {
        inputs.retain(|checkpoint| !pathing_ids.contains(&checkpoint.id));
    }
}

fn insert_resolved_goal_batch(
    goal_inputs: &mut BTreeMap<RegionKey, Vec<EntityGoalCheckpoint>>,
    batches: &mut BTreeMap<RegionKey, ResolvedGoalTick>,
    region: RegionKey,
    resolved: ResolvedGoalTick,
    pathing_checkpoints: Vec<EntityGoalCheckpoint>,
) {
    let explicit = goal_inputs.entry(region).or_default();
    if explicit.is_empty() {
        *explicit = pathing_checkpoints;
    } else if !pathing_checkpoints.is_empty() {
        let mut left = std::mem::take(explicit).into_iter().peekable();
        let mut right = pathing_checkpoints.into_iter().peekable();
        let mut merged = Vec::with_capacity(left.len() + right.len());
        while let (Some(left_checkpoint), Some(right_checkpoint)) = (left.peek(), right.peek()) {
            assert_ne!(
                left_checkpoint.id, right_checkpoint.id,
                "pathing and explicit goal checkpoints must be disjoint"
            );
            if left_checkpoint.id < right_checkpoint.id {
                merged.push(left.next().expect("peeked left checkpoint"));
            } else {
                merged.push(right.next().expect("peeked right checkpoint"));
            }
        }
        merged.extend(left);
        merged.extend(right);
        *explicit = merged;
    }
    assert!(
        batches.insert(region, resolved).is_none(),
        "resolved regional goal batches must be unique"
    );
}

#[derive(Debug)]
pub struct RegionalEntityStore {
    authority: RegionalAuthorityId,
    ownership: RegionOwnership,
    stores: BTreeMap<RegionKey, EntityStore>,
    locations: BTreeMap<EntityId, RegionKey>,
    uuids: HashMap<Uuid, EntityId>,
    transfers: BTreeMap<TransferId, PreparedTransfer>,
    in_flight_transfers: BTreeMap<EntityId, TransferId>,

    next_id: i32,
}

pub struct RegionalOwnerCoordinator {
    authority: RegionalAuthorityId,
    ownership: RegionOwnership,
    locations: BTreeMap<EntityId, RegionKey>,
    uuids: HashMap<Uuid, EntityId>,
    vehicle_passengers: HashMap<EntityId, EntityId>,
    passenger_vehicles: HashMap<EntityId, EntityId>,
    transfers: BTreeMap<TransferId, PreparedTransfer>,
    in_flight_transfers: BTreeMap<EntityId, TransferId>,

    next_id: i32,
    lanes: BTreeMap<usize, RegionalOwnerLane>,
    commit_state: Arc<RegionalOwnerCommitState>,
}

struct MutationExecution {
    goal_stats: GoalTickStats,
    goal_candidate_runs: Vec<Vec<FencedKinematicsCandidate>>,
    lane_versions: BTreeMap<usize, u64>,
    effect_results: Vec<(u64, EntityEffectResult)>,
}

struct RegionalOwnerCommitState {
    next_phase: AtomicU64,
    next_sequence: AtomicU64,
    lifecycle_epoch: AtomicU64,
    journal: Mutex<Box<dyn RegionalDecisionJournal>>,
    journal_commits: Mutex<BTreeMap<RegionPhase, (RegionPhase, u64, u64)>>,
    outcome_unknown: AtomicBool,
    #[cfg(test)]
    save_barrier_phase_snapshot_hook: Mutex<Option<SaveBarrierPhaseSnapshotHook>>,
}

type VehicleIndexes = (HashMap<EntityId, EntityId>, HashMap<EntityId, EntityId>);

#[cfg(test)]
struct SaveBarrierPhaseSnapshotHook {
    entered: SyncSender<()>,
    release: Receiver<()>,
}

impl RegionalOwnerCommitState {
    fn ensure_committed_state(&self) -> Result<(), RegionOwnerLaneError> {
        if self.outcome_unknown.load(Ordering::Acquire) {
            Err(RegionOwnerLaneError::OutcomeUnknown)
        } else {
            Ok(())
        }
    }

    fn mark_outcome_unknown(&self) {
        self.outcome_unknown.store(true, Ordering::Release);
    }

    fn reserve_sequences(&self, count: usize) -> Result<(u64, u64), RegionOwnerLaneError> {
        self.ensure_committed_state()?;
        let count = u64::try_from(count).map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        let start = self
            .next_sequence
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(count)
            })
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)?;
        Ok((start, start + count))
    }

    fn sequence_watermark(&self) -> u64 {
        self.next_sequence.load(Ordering::Acquire)
    }

    fn lifecycle_epoch(&self) -> u64 {
        self.lifecycle_epoch.load(Ordering::Acquire)
    }

    fn advance_lifecycle_epoch(&self, next: u64) -> Result<(), RegionOwnerLaneError> {
        self.lifecycle_epoch
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (next >= current).then_some(next)
            })
            .map(|_| ())
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)
    }

    fn restore_checkpoint_boundary(
        &self,
        lifecycle_epoch: u64,
        sequence_watermark: u64,
    ) -> Result<(), RegionOwnerLaneError> {
        self.ensure_committed_state()?;
        self.lifecycle_epoch
            .fetch_max(lifecycle_epoch, Ordering::AcqRel);
        self.next_sequence
            .fetch_max(sequence_watermark, Ordering::AcqRel);
        Ok(())
    }

    fn reserve_phase(&self) -> Result<RegionPhase, RegionOwnerLaneError> {
        self.ensure_committed_state()?;
        self.next_phase
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                current.checked_add(1)
            })
            .map(|phase| RegionPhase(phase + 1))
            .map_err(|_| RegionOwnerLaneError::InvalidMutation)
    }

    fn clear_recovered_commits(&self, phases: &[RegionPhase]) -> Result<(), RegionOwnerLaneError> {
        let identities = {
            let journal_commits = lock_authoritative_mutex(
                &self.journal_commits,
                "regional.journal_commit_identities",
            );
            phases
                .iter()
                .filter_map(|phase| journal_commits.get(phase).copied())
                .collect::<Vec<_>>()
        };
        lock_authoritative_mutex(&self.journal, "regional.decision_journal")
            .clear_commit_identities(&identities)
            .map_err(|_| RegionOwnerLaneError::Journal)?;
        let mut journal_commits =
            lock_authoritative_mutex(&self.journal_commits, "regional.journal_commit_identities");
        for identity in identities {
            if journal_commits.get(&identity.0) == Some(&identity) {
                journal_commits.remove(&identity.0);
            }
        }
        Ok(())
    }

    fn pending_journal_phases(&self, sequence_watermark: u64) -> Vec<RegionPhase> {
        #[cfg(test)]
        if let Some(hook) = self
            .save_barrier_phase_snapshot_hook
            .lock()
            .expect("save-barrier test hook lock poisoned")
            .take()
        {
            hook.entered
                .send(())
                .expect("save barrier phase snapshot observer dropped");
            hook.release
                .recv()
                .expect("save barrier phase snapshot release dropped");
        }
        lock_authoritative_mutex(&self.journal_commits, "regional.journal_commit_identities")
            .iter()
            .filter_map(|(&phase, identity)| (identity.1 <= sequence_watermark).then_some(phase))
            .collect()
    }

    fn record_commit(
        &self,
        decision: &RegionalCommitDecision,
    ) -> Result<(), RegionalDecisionJournalError> {
        lock_authoritative_mutex(&self.journal, "regional.decision_journal")
            .record_commit(decision)?;
        lock_authoritative_mutex(&self.journal_commits, "regional.journal_commit_identities")
            .insert(decision.phase(), decision.identity());
        Ok(())
    }

    #[cfg(test)]
    fn pause_before_save_barrier_phase_snapshot(
        &self,
        entered: SyncSender<()>,
        release: Receiver<()>,
    ) {
        *self
            .save_barrier_phase_snapshot_hook
            .lock()
            .expect("save-barrier test hook lock poisoned") =
            Some(SaveBarrierPhaseSnapshotHook { entered, release });
    }
}

#[derive(Clone)]
pub struct RegionalOwnerHandle {
    sender: SyncSender<RegionalOwnerCommand>,
    worker_health: Arc<RegionalOwnerWorkerHealth>,
    authority: RegionalAuthorityId,
    selected_read_routes: Arc<RwLock<HashMap<EntityId, CachedEntityReadRoute>>>,
    mutation_gate: Arc<RwLock<()>>,
    direct_selected_reads: Arc<AtomicUsize>,
    commit_state: Arc<RegionalOwnerCommitState>,
    #[cfg(test)]
    selected_read_probe: Arc<std::sync::Mutex<Option<SelectedReadProbe>>>,
    #[cfg(test)]
    referenced_goal_fallback_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    #[cfg(test)]
    direct_mutation_admission_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<bool>>>>,
    #[cfg(test)]
    actor_snapshot_probe: Arc<std::sync::Mutex<Option<SelectedReadProbe>>>,
    #[cfg(test)]
    goal_prepare_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    #[cfg(test)]
    goal_apply_topology_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
}

#[cfg(test)]
struct SelectedReadProbe {
    entered: std::sync::mpsc::Sender<()>,
    release: std::sync::mpsc::Receiver<()>,
}

struct DirectSelectedReadPermit {
    active: Arc<AtomicUsize>,
}

fn cached_mutation_expected_snapshots<'a>(
    fallback: &'a EntitySnapshot,
    mutation: &'a RegionOwnerMutation,
) -> &'a [EntitySnapshot] {
    match mutation {
        RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, .. } => expected,
        RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected) => expected,
        _ => std::slice::from_ref(fallback),
    }
}

fn kinematics_direct_mutations(
    states: &[(EntitySnapshot, EntityKinematics)],
) -> Result<Vec<(EntitySnapshot, RegionOwnerMutation)>, RegionOwnerLaneError> {
    if states.len() == 1 {
        let (expected, state) = &states[0];
        return Ok(vec![(
            expected.clone(),
            RegionOwnerMutation::SetKinematicsIfCurrent {
                expected: Box::new(expected.clone()),
                state: *state,
            },
        )]);
    }
    let mut states_by_region =
        BTreeMap::<RegionKey, (Vec<EntitySnapshot>, Vec<EntityKinematics>)>::new();
    for (expected, state) in states {
        let source = RegionKey::from_position(expected.position)
            .ok_or(RegionOwnerLaneError::InvalidMutation)?;
        let (expected_batch, state_batch) = states_by_region.entry(source).or_default();
        expected_batch.push(expected.clone());
        state_batch.push(*state);
    }
    Ok(states_by_region
        .into_values()
        .map(|(expected, states)| {
            (
                expected[0].clone(),
                RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, states },
            )
        })
        .collect())
}

impl DirectSelectedReadPermit {
    fn try_acquire(active: Arc<AtomicUsize>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |current| {
                (current < DIRECT_SELECTED_READ_LIMIT).then_some(current + 1)
            })
            .ok()?;
        Some(Self { active })
    }
}

impl Drop for DirectSelectedReadPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::Release);
    }
}

#[derive(Clone)]
struct CachedEntityReadRoute {
    lease: RegionLease,
    uuid: Uuid,
    standalone: bool,
    owner: RegionalOwnerLaneReader,
}

struct CachedSnapshotCommit {
    fence: RegionalVersionFence,
    snapshots: Vec<EntitySnapshot>,
}

impl CachedSnapshotCommit {
    fn into_snapshots(self) -> Vec<EntitySnapshot> {
        self.snapshots
    }

    fn into_versioned(self, authority: RegionalAuthorityId) -> VersionedEntitySnapshots {
        VersionedEntitySnapshots {
            authority,
            fence: self.fence,
            snapshots: self.snapshots,
            publication_readers: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RegionalOwnerStatus {
    pub entity_count: usize,
    pub lane_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ItemPickupClaimResolution {
    Updated(EntitySnapshot),
    Removed(EntitySnapshot),
}

#[derive(Debug, Clone)]
pub struct VersionedEntitySnapshots {
    authority: RegionalAuthorityId,
    fence: RegionalVersionFence,
    snapshots: Vec<EntitySnapshot>,
    publication_readers: Option<Arc<BTreeMap<usize, RegionalOwnerLaneReader>>>,
}

#[derive(Debug, Clone)]
/// Opaque one-use post-goal batch consumed only by the following physics apply.
pub struct VersionedEntityKinematics {
    authority: RegionalAuthorityId,
    fence: RegionalVersionFence,
    expected: Vec<CompactEntityKinematicsFence>,
    routes: Vec<RegionLease>,
    motion_states: Option<Vec<EntityMotionState>>,
    owner_readers: Option<Arc<BTreeMap<usize, RegionalOwnerLaneReader>>>,
}
impl VersionedEntityKinematics {
    /// Reports whether this one-use fence covers exactly the requested IDs.
    #[must_use]
    pub fn covers_ids(&self, mut ids: impl ExactSizeIterator<Item = EntityId>) -> bool {
        if self.expected.len() != ids.len() {
            return false;
        }
        let expected_ids = self
            .expected
            .iter()
            .map(|expected| expected.id)
            .collect::<HashSet<_>>();
        expected_ids.len() == self.expected.len() && ids.all(|id| expected_ids.contains(&id))
    }
    pub fn subset_for_states(&self, states: &[EntityKinematics]) -> Option<Self> {
        if states.is_empty() {
            return None;
        }
        let mut seen = HashSet::with_capacity(states.len());
        let mut expected = Vec::with_capacity(states.len());
        let mut routes = Vec::with_capacity(states.len());
        let mut lanes = BTreeSet::new();
        let expected_indices = self
            .expected
            .iter()
            .enumerate()
            .map(|(index, expected)| (expected.id, index))
            .collect::<HashMap<_, _>>();
        for state in states {
            if !state.is_finite() || !seen.insert(state.id) {
                return None;
            }
            let index = *expected_indices.get(&state.id)?;
            let previous = *self.expected.get(index)?;
            let lease = *self.routes.get(index)?;
            lanes.insert(lease.lane);
            routes.push(lease);
            expected.push(previous);
        }
        let mut lane_versions = BTreeMap::new();
        for lane in &lanes {
            lane_versions.insert(*lane, *self.fence.lane_versions.get(lane)?);
        }
        let owner_readers = self.owner_readers.as_ref().map(|readers| {
            lanes
                .iter()
                .map(|lane| Some((*lane, readers.get(lane)?.clone())))
                .collect::<Option<BTreeMap<_, _>>>()
        });
        let owner_readers = match owner_readers {
            Some(Some(readers)) => Some(Arc::new(readers)),
            Some(None) => return None,
            None => None,
        };
        let motion_states = match self.motion_states.as_ref() {
            Some(motion_states) => Some(
                states
                    .iter()
                    .map(|state| {
                        motion_states
                            .binary_search_by_key(&state.id, |motion| motion.id)
                            .ok()
                            .and_then(|index| motion_states.get(index))
                            .copied()
                    })
                    .collect::<Option<Vec<_>>>()?,
            ),
            None => None,
        };
        Some(Self {
            authority: self.authority,
            fence: RegionalVersionFence {
                leases: HashMap::new(),
                lane_versions,
            },
            expected,
            routes,
            motion_states,
            owner_readers,
        })
    }
    pub fn take_motion_states(&mut self) -> Option<Vec<EntityMotionState>> {
        self.motion_states.take()
    }

    #[must_use]
    pub fn motion_states(&self) -> Option<&[EntityMotionState]> {
        self.motion_states.as_deref()
    }
}

#[derive(Debug, Clone)]
pub struct VersionedKinematicsCommit {
    states: Vec<EntityKinematics>,
    fence: VersionedEntitySnapshots,
    before_lane_versions: BTreeMap<usize, u64>,
}

impl VersionedKinematicsCommit {
    #[must_use]
    pub fn states(&self) -> &[EntityKinematics] {
        &self.states
    }

    /// Retains the post-commit routing/version fence for publication CAS.
    #[must_use]
    pub fn publication_fence(&self) -> VersionedEntitySnapshots {
        self.fence.fence_only()
    }

    pub fn rebase_snapshots(&self, selected: &mut VersionedEntitySnapshots) -> bool {
        if self.fence.authority != selected.authority
            || selected
                .fence
                .lane_versions
                .iter()
                .any(|(lane, version)| self.before_lane_versions.get(lane) != Some(version))
            || selected
                .fence
                .lane_versions
                .keys()
                .any(|lane| !self.fence.fence.lane_versions.contains_key(lane))
            || self.states.iter().any(|state| {
                selected
                    .snapshots
                    .binary_search_by_key(&state.id, |snapshot| snapshot.id)
                    .is_err()
            })
        {
            return false;
        }
        for (lane, version) in &mut selected.fence.lane_versions {
            *version = self.fence.fence.lane_versions[lane];
        }
        for state in &self.states {
            let index = selected
                .snapshots
                .binary_search_by_key(&state.id, |snapshot| snapshot.id)
                .expect("kinematics snapshot ids were prevalidated");
            let snapshot = &mut selected.snapshots[index];
            snapshot.position = state.position;
            snapshot.rotation = state.rotation;
            snapshot.velocity = state.velocity;
            snapshot.on_ground = state.on_ground;
        }
        true
    }

    pub fn into_states(self) -> Vec<EntityKinematics> {
        self.states
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
/// Compact internal identity and motion fence; fields intentionally stay opaque.
pub struct CompactEntityKinematicsFence {
    id: EntityId,
    uuid: Uuid,
    lifecycle: crate::EntityLifecycle,
    expected: EntityKinematics,
    pickup_claimed: bool,
    vehicle_attached: bool,
}

impl CompactEntityKinematicsFence {
    fn eligible(self) -> bool {
        self.lifecycle == crate::EntityLifecycle::Alive
            && !self.pickup_claimed
            && !self.vehicle_attached
            && self.id == self.expected.id
    }
}
#[derive(Debug, Clone, Copy)]
struct CompactEntityPhysicsFence {
    id: EntityId,
    uuid: Uuid,
    lifecycle: crate::EntityLifecycle,
    rotation: crate::Rotation,
    pickup_claimed: bool,
    vehicle_attached: bool,
}

impl From<CompactEntityKinematicsFence> for CompactEntityPhysicsFence {
    fn from(fence: CompactEntityKinematicsFence) -> Self {
        Self {
            id: fence.id,
            uuid: fence.uuid,
            lifecycle: fence.lifecycle,
            rotation: fence.expected.rotation,
            pickup_claimed: fence.pickup_claimed,
            vehicle_attached: fence.vehicle_attached,
        }
    }
}

impl VersionedEntitySnapshots {
    #[must_use]
    pub fn snapshots(&self) -> &[EntitySnapshot] {
        &self.snapshots
    }

    /// Reports whether this fence covers exactly the requested entity IDs.
    #[must_use]
    pub fn covers_ids(&self, mut ids: impl ExactSizeIterator<Item = EntityId>) -> bool {
        self.fence.leases.len() == ids.len() && ids.all(|id| self.fence.leases.contains_key(&id))
    }

    #[must_use]
    pub fn kinematics_fence(&self) -> Option<VersionedEntityKinematics> {
        if self.snapshots.is_empty() {
            return None;
        }
        Some(VersionedEntityKinematics {
            authority: self.authority,
            fence: self.fence.clone(),
            routes: self
                .snapshots
                .iter()
                .map(|snapshot| self.fence.leases.get(&snapshot.id).copied())
                .collect::<Option<Vec<_>>>()?,
            expected: self
                .snapshots
                .iter()
                .map(|snapshot| CompactEntityKinematicsFence {
                    id: snapshot.id,
                    uuid: snapshot.uuid,
                    lifecycle: snapshot.lifecycle,
                    expected: EntityKinematics {
                        id: snapshot.id,
                        position: snapshot.position,
                        rotation: snapshot.rotation,
                        velocity: snapshot.velocity,
                        on_ground: snapshot.on_ground,
                    },
                    pickup_claimed: snapshot.retained.item_pickup_claim.is_some(),
                    vehicle_attached: snapshot.vehicle.is_some(),
                })
                .collect(),
            motion_states: None,
            owner_readers: None,
        })
    }

    /// Retain only the routing/version fence after snapshot consumers finish.
    ///
    /// The full snapshots must remain present until any kinematics rebase has
    /// completed. This is used only for the publication CAS fence, which never
    /// reads the snapshot payload.
    #[must_use]
    pub fn without_snapshots(self) -> Self {
        Self {
            authority: self.authority,
            fence: self.fence,
            snapshots: Vec::new(),
            publication_readers: self.publication_readers,
        }
    }
    #[must_use]
    pub fn fence_only(&self) -> Self {
        Self {
            authority: self.authority,
            fence: self.fence.clone(),
            snapshots: Vec::new(),
            publication_readers: self.publication_readers.clone(),
        }
    }

    /// Consume the publication and return its authoritative snapshots.
    pub fn into_snapshots(self) -> Vec<EntitySnapshot> {
        self.snapshots
    }
}

#[derive(Debug, Clone)]
struct RegionalVersionFence {
    leases: HashMap<EntityId, RegionLease>,
    lane_versions: BTreeMap<usize, u64>,
}

struct SelectedEntitySnapshots {
    fence: RegionalVersionFence,
    snapshots: Vec<EntitySnapshot>,
}

const OWNER_WORKER_RUNNING: u8 = 0;
const OWNER_WORKER_STOPPED: u8 = 1;
const OWNER_WORKER_PANICKED: u8 = 2;

#[derive(Debug)]
struct RegionalOwnerWorkerHealth {
    state: AtomicU8,
}

impl RegionalOwnerWorkerHealth {
    fn new() -> Self {
        Self {
            state: AtomicU8::new(OWNER_WORKER_RUNNING),
        }
    }

    fn mark_stopped(&self) {
        self.state.store(OWNER_WORKER_STOPPED, Ordering::Release);
    }

    fn mark_panicked(&self) {
        self.state.store(OWNER_WORKER_PANICKED, Ordering::Release);
    }

    fn error(&self) -> Option<RegionOwnerLaneError> {
        match self.state.load(Ordering::Acquire) {
            OWNER_WORKER_RUNNING => None,
            OWNER_WORKER_PANICKED => Some(RegionOwnerLaneError::WorkerPanicked),
            _ => Some(RegionOwnerLaneError::Closed),
        }
    }

    fn error_after_disconnect(&self) -> RegionOwnerLaneError {
        loop {
            if let Some(error) = self.error() {
                return error;
            }
            std::thread::yield_now();
        }
    }
}

pub struct RegionalOwnerRuntime {
    handle: RegionalOwnerHandle,
    worker: Option<JoinHandle<()>>,
}

#[derive(Debug)]
pub enum RegionalOwnerRuntimeShutdownError {
    Closed,
    WorkerPanicked,
    Owner(RegionalOwnerShutdownError),
}

type GoalApplySimulationResult = Result<
    Option<(
        GoalTickStats,
        Vec<EntitySimulationResult>,
        Option<VersionedEntityKinematics>,
    )>,
    RegionOwnerLaneError,
>;

type GoalApplyKinematicsResult = Result<
    Option<(
        GoalTickStats,
        Vec<EntityKinematics>,
        Option<VersionedEntityKinematics>,
    )>,
    RegionOwnerLaneError,
>;
type VersionedKinematicsRead = (Vec<EntityKinematics>, Option<VersionedEntityKinematics>);
type VersionedSimulationResultsRead = (
    Vec<EntitySimulationResult>,
    Option<VersionedEntityKinematics>,
);
type SelectedPhysicsCommit = (
    Vec<EntityId>,
    Vec<EntityMotionState>,
    Vec<EntityKinematics>,
    VersionedEntitySnapshots,
);
type FencedSimulationResults = (
    Vec<EntityKinematics>,
    Vec<EntitySimulationResult>,
    Option<VersionedEntityKinematics>,
);

#[derive(Clone)]
enum SnapshotSelection {
    All,
    SheepGrazing {
        include_idle: Arc<HashSet<EntityId>>,
    },
}

/// A regional mutation prepared before an external durable participant commits.
/// Drop waits for abort acknowledgement; after the external decision commits,
/// `commit` never aborts on an uncertain regional journal outcome.
pub struct PreparedSnapshotMutation {
    owner: RegionalOwnerHandle,
    phase: RegionPhase,
    lane: usize,
    sequence: u64,
    entity: EntityId,
    committed: bool,
}

impl std::fmt::Debug for PreparedSnapshotMutation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreparedSnapshotMutation")
            .field("phase", &self.phase)
            .field("entity", &self.entity)
            .finish()
    }
}

impl PreparedSnapshotMutation {
    pub fn commit(mut self) -> Result<(), RegionOwnerLaneError> {
        let (reply, result) = channel();
        self.owner
            .sender
            .send(RegionalOwnerCommand::CommitPreparedSnapshot {
                phase: self.phase,
                lane: self.lane,
                sequence: self.sequence,
                entity: self.entity,
                reply,
            })
            .map_err(|_| self.owner.unavailable_error())?;
        self.committed = true;
        result.recv().map_err(|_| self.owner.unavailable_error())?
    }
    /// The external journal's outcome is unknown; keep the phase fenced until
    /// process recovery rather than admitting a conflicting native mutation.
    pub fn leave_fenced_for_recovery(mut self) {
        self.committed = true;
    }
}

impl Drop for PreparedSnapshotMutation {
    fn drop(&mut self) {
        if !self.committed {
            let (reply, result) = channel();
            if self
                .owner
                .sender
                .send(RegionalOwnerCommand::AbortPreparedSnapshot {
                    phase: self.phase,
                    lane: self.lane,
                    reply,
                })
                .is_ok()
            {
                let _ = result.recv();
            }
        }
    }
}

type PreparedSnapshotReply = Result<Option<(RegionPhase, usize, u64)>, RegionOwnerLaneError>;

enum RegionalOwnerCommand {
    Snapshot {
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    Snapshots {
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    SnapshotsForIds {
        entities: HashSet<EntityId>,
        selection: SnapshotSelection,
        reply: std::sync::mpsc::Sender<Result<VersionedEntitySnapshots, RegionOwnerLaneError>>,
    },
    AliveKinematicsForIds {
        entities: HashSet<EntityId>,
        reply: std::sync::mpsc::Sender<Result<VersionedKinematicsRead, RegionOwnerLaneError>>,
    },
    SimulationResultsForIds {
        entities: HashSet<EntityId>,
        reply:
            std::sync::mpsc::Sender<Result<VersionedSimulationResultsRead, RegionOwnerLaneError>>,
    },
    SimulationProjectionsForIds {
        entities: HashSet<EntityId>,
        reply:
            std::sync::mpsc::Sender<Result<Vec<EntitySimulationProjection>, RegionOwnerLaneError>>,
    },
    DespawnProjectionsForIds {
        entities: HashSet<EntityId>,
        reply: std::sync::mpsc::Sender<
            Result<Vec<crate::EntityDespawnProjection>, RegionOwnerLaneError>,
        >,
    },
    ContainsUuid {
        uuid: Uuid,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    EntityByUuid {
        uuids: Vec<Uuid>,
        reply: std::sync::mpsc::Sender<Result<Vec<Option<EntitySnapshot>>, RegionOwnerLaneError>>,
    },
    Status {
        reply: std::sync::mpsc::Sender<Result<RegionalOwnerStatus, RegionOwnerLaneError>>,
    },
    ReconfigureLanes {
        lane_count: usize,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    SaveBarrier {
        reply: std::sync::mpsc::Sender<Result<RegionalOwnerSaveSnapshot, RegionOwnerLaneError>>,
    },
    AdvanceLifecycleEpoch {
        lifecycle_epoch: u64,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    RestoreCheckpointBoundary {
        lifecycle_epoch: u64,
        sequence_watermark: u64,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    Spawn {
        entity: Box<SpawnEntity>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<EntityId, RegionOwnerLaneError>>,
    },
    SpawnBatch {
        entities: Vec<SpawnEntity>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntityId>, RegionOwnerLaneError>>,
    },
    SpawnUniqueBatch {
        entities: Vec<SpawnEntity>,
        reply: std::sync::mpsc::Sender<Result<Vec<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    InsertSnapshots {
        snapshots: Vec<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    Remove {
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    RemoveIfCurrent {
        expected: Box<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    ReplaceSnapshotIfCurrent {
        expected: Box<EntitySnapshot>,
        next: Box<EntitySnapshot>,
        defer_journal: bool,
        allow_type_change: bool,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    PrepareSnapshot {
        expected: Box<EntitySnapshot>,
        next: Box<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<PreparedSnapshotReply>,
    },
    CommitPreparedSnapshot {
        phase: RegionPhase,
        lane: usize,
        sequence: u64,
        entity: EntityId,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    AbortPreparedSnapshot {
        phase: RegionPhase,
        lane: usize,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    ReplaceSnapshotsIfCurrent {
        snapshots: Vec<(EntitySnapshot, EntitySnapshot)>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    MergeItemSnapshotsIfCurrent {
        survivor_expected: Box<EntitySnapshot>,
        survivor_next: Box<EntitySnapshot>,
        consumed_expected: Box<EntitySnapshot>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    CommitVillagerInventoryPickupIfCurrent {
        commit: Box<VillagerInventoryPickupCommit>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    CommitVillagerFoodShareIfCurrent {
        commit: Box<VillagerFoodShareCommit>,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    CommitVillagerBirthIfCurrent {
        commit: Box<VillagerBirthCommit>,
        reply: std::sync::mpsc::Sender<Result<Option<EntitySnapshot>, RegionOwnerLaneError>>,
    },
    CommitVillagerCourtshipIfCurrent {
        commit: Box<VillagerCourtshipCommit>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    CommitVillagerNoBedIfCurrent {
        commit: Box<VillagerNoBedCommit>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetAnimalStatesIfCurrent {
        states: Vec<(EntitySnapshot, AnimalBreedingState)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetGoal {
        entity: EntityId,
        goal: GoalState,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetGoals {
        goals: Vec<(EntityId, GoalState)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<usize, RegionOwnerLaneError>>,
    },
    SetItemStack {
        entity: EntityId,
        item_stack: Option<EntityItemStack>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetItemStackIfCurrent {
        expected: Box<EntitySnapshot>,
        item_stack: Option<EntityItemStack>,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    ResolveItemPickupClaim {
        entity: EntityId,
        claim: u64,
        item_stack: Option<EntityItemStack>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<
            Result<Option<ItemPickupClaimResolution>, RegionOwnerLaneError>,
        >,
    },
    SetPosition {
        entity: EntityId,
        position: Vec3,
        reply: std::sync::mpsc::Sender<Result<bool, RegionOwnerLaneError>>,
    },
    SetVelocities {
        velocities: Vec<(EntityId, Vec3)>,
        reply: std::sync::mpsc::Sender<Result<(), RegionOwnerLaneError>>,
    },
    ApplyKinematicsIfCurrent {
        states: Vec<(EntitySnapshot, EntityKinematics)>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<Result<VersionedEntitySnapshots, RegionOwnerLaneError>>,
    },
    ApplyKinematicsIfVersioned {
        expected: VersionedEntityKinematics,
        states: Vec<EntityKinematics>,
        reply: std::sync::mpsc::Sender<
            Result<Option<VersionedKinematicsCommit>, RegionOwnerLaneError>,
        >,
    },
    DamageIfCurrent {
        expected: Box<EntitySnapshot>,
        request: EntityDamageRequest,
        reply: std::sync::mpsc::Sender<Result<Option<EntityDamage>, RegionOwnerLaneError>>,
    },
    ApplyEffectIfCurrent {
        expected: Box<EntitySnapshot>,
        request: Box<EntityEffectRequest>,
        reply: std::sync::mpsc::Sender<Result<EntityEffectResult, RegionOwnerLaneError>>,
    },
    Damage {
        entity: EntityId,
        request: EntityDamageRequest,
        reply: std::sync::mpsc::Sender<Result<Option<EntityDamage>, RegionOwnerLaneError>>,
    },
    PrepareGoalTick {
        tick: u64,
        active_ids: Arc<HashSet<EntityId>>,
        selected: Option<VersionedEntitySnapshots>,
        inputs: RegionalGoalTickInputs,
        reply: std::sync::mpsc::Sender<Result<RegionalPreparedGoalTick, RegionOwnerLaneError>>,
    },
    ApplyPreparedGoalTick {
        resolved: Box<RegionalResolvedGoalTick>,
        reply: std::sync::mpsc::Sender<Result<GoalTickStats, RegionOwnerLaneError>>,
    },
    ApplyPreparedGoalTickAndKinematics {
        resolved: Box<RegionalResolvedGoalTick>,
        entities: Arc<HashSet<EntityId>>,
        defer_journal: bool,
        reply: std::sync::mpsc::Sender<GoalApplySimulationResult>,
    },

    #[cfg(test)]
    HoldForTest {
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    },
    TickOwnedRegions {
        input: RegionalEntityTickInput,
        reply: std::sync::mpsc::Sender<Result<RegionalEntityTickOutput, RegionOwnerLaneError>>,
    },
    CommitOwnedRegionPhysics {
        prepared: Box<RegionalPreparedEntityPhysics>,
        reply: std::sync::mpsc::Sender<Result<RegionalEntityPhysicsOutput, RegionOwnerLaneError>>,
    },
    #[cfg(test)]
    HoldLaneForTest {
        entity: EntityId,
        entered: std::sync::mpsc::Sender<()>,
        release: Receiver<()>,
    },
    #[cfg(test)]
    PanicForTest,
    Shutdown {
        reply: std::sync::mpsc::Sender<Result<RegionalEntityStore, RegionalOwnerShutdownError>>,
    },
}

enum RegionalOwnerLaneScope {
    Entities(Vec<EntityId>),
    EntitiesAndRegions {
        entities: Vec<EntityId>,
        regions: Vec<RegionKey>,
    },
    All,
    TopologyOnly,
}

enum RegionalOwnerTopologyAccess {
    Unfenced,
    Shared(Vec<RegionalOwnerLaneReader>),
    Exclusive,
}

impl RegionalOwnerCommand {
    fn lane_scope(&self) -> Option<RegionalOwnerLaneScope> {
        match self {
            Self::Snapshot { entity, .. } => Some(RegionalOwnerLaneScope::Entities(vec![*entity])),
            Self::Snapshots { .. } => Some(RegionalOwnerLaneScope::All),
            Self::EntityByUuid { .. } => Some(RegionalOwnerLaneScope::All),
            Self::SnapshotsForIds { entities, .. } => Some(RegionalOwnerLaneScope::Entities(
                entities.iter().copied().collect(),
            )),
            Self::AliveKinematicsForIds { entities, .. } => Some(RegionalOwnerLaneScope::Entities(
                entities.iter().copied().collect(),
            )),
            Self::SimulationResultsForIds { entities, .. } => Some(
                RegionalOwnerLaneScope::Entities(entities.iter().copied().collect()),
            ),
            Self::SetAnimalStatesIfCurrent { states, .. } => {
                Some(RegionalOwnerLaneScope::Entities(
                    states.iter().map(|(snapshot, _)| snapshot.id).collect(),
                ))
            }
            Self::SetGoal { entity, goal, .. } => Some(RegionalOwnerLaneScope::Entities(
                std::iter::once(*entity)
                    .chain(goal_reference(goal))
                    .collect(),
            )),
            Self::SetGoals { goals, .. } => Some(RegionalOwnerLaneScope::Entities(
                goals
                    .iter()
                    .flat_map(|(entity, goal)| std::iter::once(*entity).chain(goal_reference(goal)))
                    .collect(),
            )),
            Self::SetItemStack { entity, .. }
            | Self::ResolveItemPickupClaim { entity, .. }
            | Self::Damage { entity, .. } => Some(RegionalOwnerLaneScope::Entities(vec![*entity])),
            Self::SetItemStackIfCurrent { expected, .. }
            | Self::DamageIfCurrent { expected, .. }
            | Self::ApplyEffectIfCurrent { expected, .. } => {
                Some(RegionalOwnerLaneScope::Entities(vec![expected.id]))
            }
            Self::SetVelocities { velocities, .. } => Some(RegionalOwnerLaneScope::Entities(
                velocities.iter().map(|(entity, _)| *entity).collect(),
            )),
            Self::ApplyKinematicsIfVersioned { expected, .. } => {
                Some(RegionalOwnerLaneScope::Entities(
                    expected.expected.iter().map(|entry| entry.id).collect(),
                ))
            }
            Self::PrepareGoalTick { .. } => Some(RegionalOwnerLaneScope::TopologyOnly),
            Self::ApplyPreparedGoalTick { resolved, .. } => {
                Some(goal_apply_lane_scope(resolved, None))
            }
            Self::ApplyPreparedGoalTickAndKinematics {
                resolved, entities, ..
            } => Some(goal_apply_lane_scope(resolved, Some(entities))),
            Self::TickOwnedRegions { .. } | Self::CommitOwnedRegionPhysics { .. } => {
                Some(RegionalOwnerLaneScope::TopologyOnly)
            }
            #[cfg(test)]
            Self::HoldLaneForTest { entity, .. } => {
                Some(RegionalOwnerLaneScope::Entities(vec![*entity]))
            }
            _ => None,
        }
    }

    fn mutates_entity_snapshots(&self) -> bool {
        matches!(
            self,
            Self::Spawn { .. }
                | Self::SpawnBatch { .. }
                | Self::SpawnUniqueBatch { .. }
                | Self::InsertSnapshots { .. }
                | Self::Remove { .. }
                | Self::RemoveIfCurrent { .. }
                | Self::ReplaceSnapshotIfCurrent { .. }
                | Self::PrepareSnapshot { .. }
                | Self::CommitPreparedSnapshot { .. }
                | Self::AbortPreparedSnapshot { .. }
                | Self::ReplaceSnapshotsIfCurrent { .. }
                | Self::MergeItemSnapshotsIfCurrent { .. }
                | Self::SetAnimalStatesIfCurrent { .. }
                | Self::SetGoal { .. }
                | Self::SetGoals { .. }
                | Self::SetItemStack { .. }
                | Self::SetItemStackIfCurrent { .. }
                | Self::ResolveItemPickupClaim { .. }
                | Self::SetPosition { .. }
                | Self::SetVelocities { .. }
                | Self::ApplyKinematicsIfCurrent { .. }
                | Self::ApplyKinematicsIfVersioned { .. }
                | Self::DamageIfCurrent { .. }
                | Self::ApplyEffectIfCurrent { .. }
                | Self::Damage { .. }
                | Self::ApplyPreparedGoalTick { .. }
                | Self::ApplyPreparedGoalTickAndKinematics { .. }
                | Self::TickOwnedRegions { .. }
                | Self::CommitOwnedRegionPhysics { .. }
        )
    }

    fn requires_exclusive_lane_access(&self) -> bool {
        self.mutates_entity_snapshots()
            || matches!(
                self,
                Self::ReconfigureLanes { .. } | Self::SaveBarrier { .. } | Self::Shutdown { .. }
            )
    }
}

fn owner_readers_for_scope(
    coordinator: &RegionalOwnerCoordinator,
    scope: &RegionalOwnerLaneScope,
) -> Result<Vec<RegionalOwnerLaneReader>, RegionOwnerLaneError> {
    let (entities, regions): (&[EntityId], &[RegionKey]) = match scope {
        RegionalOwnerLaneScope::All => {
            return Ok(coordinator
                .lanes
                .values()
                .map(RegionalOwnerLane::reader)
                .collect());
        }
        RegionalOwnerLaneScope::TopologyOnly => return Ok(Vec::new()),
        RegionalOwnerLaneScope::Entities(entities) => (entities, &[]),
        RegionalOwnerLaneScope::EntitiesAndRegions { entities, regions } => (entities, regions),
    };
    let mut readers = BTreeMap::new();
    for entity in entities {
        let Some(key) = coordinator.locations.get(entity).copied() else {
            continue;
        };
        let lease = coordinator
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let owner = coordinator
            .lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?;
        readers.entry(lease.lane).or_insert_with(|| owner.reader());
    }
    for key in regions {
        let lease = coordinator
            .ownership
            .lease(*key)
            .ok_or(RegionOwnerLaneError::StaleLease)?;
        let owner = coordinator
            .lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?;
        readers.entry(lease.lane).or_insert_with(|| owner.reader());
    }
    Ok(readers.into_values().collect())
}

fn goal_apply_lane_scope(
    resolved: &RegionalResolvedGoalTick,
    extra_entities: Option<&HashSet<EntityId>>,
) -> RegionalOwnerLaneScope {
    let prepared_len = resolved
        .simulation_ids
        .values()
        .map(Vec::len)
        .sum::<usize>();
    let entities = extra_entities
        .filter(|entities| {
            prepared_len != entities.len()
                || resolved
                    .simulation_ids
                    .values()
                    .flatten()
                    .any(|entity| !entities.contains(entity))
        })
        .map_or_else(Vec::new, |entities| entities.iter().copied().collect());
    // Valid goal applies already fence every mutated and followed-target
    // region. Route only those few regions; malformed combined commands retain
    // their explicit entity scope so topology failure remains conservative.
    let regions = resolved
        .leases
        .keys()
        .chain(resolved.batches.keys())
        .copied()
        .chain(
            resolved
                .follow_target_sources
                .values()
                .map(|source| source.region),
        )
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    RegionalOwnerLaneScope::EntitiesAndRegions { entities, regions }
}

fn topology_access_for_command(
    coordinator: &RegionalOwnerCoordinator,
    command: &RegionalOwnerCommand,
) -> RegionalOwnerTopologyAccess {
    match command.lane_scope() {
        Some(scope) => match owner_readers_for_scope(coordinator, &scope) {
            Ok(readers) => RegionalOwnerTopologyAccess::Shared(readers),
            Err(_) => RegionalOwnerTopologyAccess::Exclusive,
        },
        None if command.requires_exclusive_lane_access() => RegionalOwnerTopologyAccess::Exclusive,
        None => RegionalOwnerTopologyAccess::Unfenced,
    }
}

impl RegionalOwnerRuntime {
    pub fn from_store(
        store: RegionalEntityStore,
        requested_lanes: usize,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        Self::from_store_with_journal(
            store,
            requested_lanes,
            Box::new(NoopRegionalDecisionJournal),
        )
    }

    pub fn from_store_with_journal(
        store: RegionalEntityStore,
        requested_lanes: usize,
        journal: Box<dyn RegionalDecisionJournal>,
    ) -> Result<Self, RegionalOwnerCutoverError> {
        let coordinator =
            RegionalOwnerCoordinator::from_store_with_journal(store, requested_lanes, journal)?;
        let authority = coordinator.authority;
        let commit_state = Arc::clone(&coordinator.commit_state);
        let selected_read_routes = Arc::new(RwLock::new(HashMap::new()));
        let runtime_selected_read_routes = Arc::clone(&selected_read_routes);
        let mutation_gate = Arc::new(RwLock::new(()));
        let runtime_mutation_gate = Arc::clone(&mutation_gate);
        let direct_selected_reads = Arc::new(AtomicUsize::new(0));
        #[cfg(test)]
        let selected_read_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let referenced_goal_fallback_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let direct_mutation_admission_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let actor_snapshot_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let runtime_actor_snapshot_probe = Arc::clone(&actor_snapshot_probe);
        #[cfg(test)]
        let goal_prepare_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let runtime_goal_prepare_probe = Arc::clone(&goal_prepare_probe);
        #[cfg(test)]
        let goal_apply_topology_probe = Arc::new(std::sync::Mutex::new(None));
        #[cfg(test)]
        let runtime_goal_apply_topology_probe = Arc::clone(&goal_apply_topology_probe);
        let (sender, receiver) = sync_channel(64);
        let (start, started) = channel();
        let worker_health = Arc::new(RegionalOwnerWorkerHealth::new());
        let runtime_worker_health = Arc::clone(&worker_health);
        let worker = match std::thread::Builder::new()
            .name("solaris-region-coordinator".to_owned())
            .spawn(move || {
                let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    run_regional_owner_runtime(
                        &started,
                        &receiver,
                        runtime_selected_read_routes,
                        runtime_mutation_gate,
                        #[cfg(test)]
                        runtime_actor_snapshot_probe,
                        #[cfg(test)]
                        runtime_goal_prepare_probe,
                        #[cfg(test)]
                        runtime_goal_apply_topology_probe,
                    );
                }));
                if outcome.is_err() {
                    runtime_worker_health.mark_panicked();
                } else {
                    runtime_worker_health.mark_stopped();
                }
            }) {
            Ok(worker) => worker,
            Err(_) => {
                return Err(recover_owner_runtime_start(
                    coordinator,
                    RegionOwnerLaneError::SpawnFailed,
                ));
            }
        };
        if let Err(error) = start.send(coordinator) {
            let _ = worker.join();
            return Err(recover_owner_runtime_start(
                error.0,
                RegionOwnerLaneError::Closed,
            ));
        }
        Ok(Self {
            handle: RegionalOwnerHandle {
                sender,
                worker_health,
                authority,
                selected_read_routes,
                mutation_gate,
                direct_selected_reads,
                commit_state,
                #[cfg(test)]
                selected_read_probe,
                #[cfg(test)]
                referenced_goal_fallback_probe,
                #[cfg(test)]
                direct_mutation_admission_probe,
                #[cfg(test)]
                actor_snapshot_probe,
                #[cfg(test)]
                goal_prepare_probe,
                #[cfg(test)]
                goal_apply_topology_probe,
            },
            worker: Some(worker),
        })
    }

    #[must_use]
    pub fn handle(&self) -> RegionalOwnerHandle {
        self.handle.clone()
    }

    pub fn shutdown(mut self) -> Result<RegionalEntityStore, RegionalOwnerRuntimeShutdownError> {
        self.stop()
    }

    fn stop(&mut self) -> Result<RegionalEntityStore, RegionalOwnerRuntimeShutdownError> {
        let Some(worker) = self.worker.take() else {
            return Err(RegionalOwnerRuntimeShutdownError::Closed);
        };
        let (reply, result) = channel();
        if self
            .handle
            .sender
            .send(RegionalOwnerCommand::Shutdown { reply })
            .is_err()
        {
            let joined = worker.join();
            return if joined.is_err()
                || self.handle.fatal_error() == Some(RegionOwnerLaneError::WorkerPanicked)
            {
                Err(RegionalOwnerRuntimeShutdownError::WorkerPanicked)
            } else {
                Err(RegionalOwnerRuntimeShutdownError::Closed)
            };
        }
        let result = result.recv();
        if worker.join().is_err()
            || self.handle.fatal_error() == Some(RegionOwnerLaneError::WorkerPanicked)
        {
            return Err(RegionalOwnerRuntimeShutdownError::WorkerPanicked);
        }
        result
            .map_err(|_| RegionalOwnerRuntimeShutdownError::Closed)?
            .map_err(RegionalOwnerRuntimeShutdownError::Owner)
    }
}

impl Drop for RegionalOwnerRuntime {
    fn drop(&mut self) {
        if self.worker.is_some() {
            let _ = self.stop();
        }
    }
}

fn run_regional_owner_runtime(
    started: &Receiver<RegionalOwnerCoordinator>,
    receiver: &Receiver<RegionalOwnerCommand>,
    selected_read_routes: Arc<RwLock<HashMap<EntityId, CachedEntityReadRoute>>>,
    mutation_gate: Arc<RwLock<()>>,
    #[cfg(test)] actor_snapshot_probe: Arc<std::sync::Mutex<Option<SelectedReadProbe>>>,
    #[cfg(test)] goal_prepare_probe: Arc<std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>>,
    #[cfg(test)] goal_apply_topology_probe: Arc<
        std::sync::Mutex<Option<std::sync::mpsc::Sender<()>>>,
    >,
) {
    let Ok(mut coordinator) = started.recv() else {
        return;
    };
    while let Ok(command) = receiver.recv() {
        let topology_access = topology_access_for_command(&coordinator, &command);
        let _shared_topology = matches!(topology_access, RegionalOwnerTopologyAccess::Shared(_))
            .then(|| read_authoritative_rwlock(&mutation_gate, "regional.mutation_gate"));
        let _exclusive_topology = matches!(topology_access, RegionalOwnerTopologyAccess::Exclusive)
            .then(|| write_authoritative_rwlock(&mutation_gate, "regional.mutation_gate"));
        #[cfg(test)]
        if matches!(
            command,
            RegionalOwnerCommand::ApplyPreparedGoalTick { .. }
                | RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics { .. }
        ) && let Some(probe) = goal_apply_topology_probe
            .lock()
            .expect("goal-apply topology test probe lock poisoned")
            .take()
        {
            let _ = probe.send(());
        }
        let _lane_admissions = match &topology_access {
            RegionalOwnerTopologyAccess::Shared(readers) => Some(
                readers
                    .iter()
                    .map(|owner| {
                        lock_authoritative_mutex(owner.admission(), "regional.owner_lane_admission")
                    })
                    .collect::<Vec<_>>(),
            ),
            RegionalOwnerTopologyAccess::Unfenced | RegionalOwnerTopologyAccess::Exclusive => None,
        };
        match command {
            RegionalOwnerCommand::Snapshot { entity, reply } => {
                let result = coordinator.snapshot(entity);
                match &result {
                    Ok(Some(snapshot)) => publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &HashSet::from([entity]),
                        std::slice::from_ref(snapshot),
                    ),
                    Ok(None) => {
                        invalidate_selected_read_routes(&selected_read_routes, [entity]);
                    }
                    Err(_) => {}
                }
                #[cfg(test)]
                if let Some(probe) = actor_snapshot_probe
                    .lock()
                    .expect("actor-snapshot test probe lock poisoned")
                    .take()
                {
                    let _ = probe.entered.send(());
                    let _ = probe.release.recv();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::Snapshots { reply } => {
                let result = coordinator.snapshots();
                if let Ok(snapshots) = &result {
                    let entities = snapshots
                        .iter()
                        .map(|snapshot| snapshot.id)
                        .collect::<HashSet<_>>();
                    publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        snapshots,
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SnapshotsForIds {
                entities,
                selection,
                reply,
            } => {
                let result = coordinator
                    .snapshots_for_ids_selected(&entities, &selection)
                    .and_then(|snapshots| {
                        let fence = selected_version_fence(&coordinator, &snapshots)?;
                        Ok(VersionedEntitySnapshots {
                            authority: coordinator.authority,
                            fence,
                            snapshots,
                            publication_readers: None,
                        })
                    });
                if matches!(selection, SnapshotSelection::All)
                    && let Ok(selected) = &result
                {
                    publish_selected_read_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        selected.snapshots(),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::AliveKinematicsForIds { entities, reply } => {
                let result = coordinator.alive_kinematics_for_ids(&entities);
                if let Ok((_, Some(selected))) = &result {
                    publish_selected_kinematics_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        selected,
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SimulationResultsForIds { entities, reply } => {
                let result = coordinator.simulation_results_for_ids(&entities);
                if let Ok((_, Some(selected))) = &result {
                    publish_selected_kinematics_routes(
                        &coordinator,
                        &selected_read_routes,
                        &entities,
                        selected,
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SimulationProjectionsForIds { entities, reply } => {
                let _ = reply.send(coordinator.simulation_projections_for_ids(&entities));
            }
            RegionalOwnerCommand::DespawnProjectionsForIds { entities, reply } => {
                let _ = reply.send(coordinator.despawn_projections_for_ids(&entities));
            }
            RegionalOwnerCommand::ContainsUuid { uuid, reply } => {
                let _ = reply.send(
                    coordinator
                        .commit_state
                        .ensure_committed_state()
                        .map(|()| coordinator.contains_uuid(uuid)),
                );
            }
            RegionalOwnerCommand::EntityByUuid { uuids, reply } => {
                let _ = reply.send(
                    coordinator
                        .commit_state
                        .ensure_committed_state()
                        .and_then(|()| coordinator.entities_by_uuid(&uuids)),
                );
            }
            RegionalOwnerCommand::Status { reply } => {
                let _ = reply.send(
                    coordinator
                        .commit_state
                        .ensure_committed_state()
                        .map(|()| coordinator.status()),
                );
            }
            RegionalOwnerCommand::ReconfigureLanes { lane_count, reply } => {
                let result = coordinator.reconfigure_lanes(lane_count);
                if result.is_ok() {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SaveBarrier { reply } => {
                let _ = reply.send(coordinator.save_barrier());
            }
            RegionalOwnerCommand::AdvanceLifecycleEpoch {
                lifecycle_epoch,
                reply,
            } => {
                let result = coordinator
                    .commit_state
                    .advance_lifecycle_epoch(lifecycle_epoch);
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::RestoreCheckpointBoundary {
                lifecycle_epoch,
                sequence_watermark,
                reply,
            } => {
                let result = coordinator
                    .commit_state
                    .restore_checkpoint_boundary(lifecycle_epoch, sequence_watermark);
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::Spawn {
                entity,
                defer_journal,
                reply,
            } => {
                let passenger = entity_vehicle_reference(&entity);
                let result = coordinator.spawn_inner(*entity, !defer_journal);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passenger.into_iter());
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SpawnBatch { entities, reply } => {
                let passengers = entities
                    .iter()
                    .filter_map(entity_vehicle_reference)
                    .collect::<Vec<_>>();
                let result = coordinator.spawn_batch(entities);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passengers);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SpawnUniqueBatch { entities, reply } => {
                let result = coordinator.spawn_unique_batch(entities);
                if let Ok(snapshots) = &result {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        snapshots.iter().filter_map(snapshot_vehicle_reference),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::InsertSnapshots { snapshots, reply } => {
                let passengers = snapshots
                    .iter()
                    .filter_map(snapshot_vehicle_reference)
                    .collect::<Vec<_>>();
                let result = coordinator.insert_snapshots_batch(snapshots);
                if result.is_ok() {
                    invalidate_selected_read_routes(&selected_read_routes, passengers);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::Remove { entity, reply } => {
                let result = coordinator.remove(entity);
                if let Ok(Some(snapshot)) = &result {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        std::iter::once(entity).chain(snapshot_vehicle_reference(snapshot)),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::RemoveIfCurrent { expected, reply } => {
                let entity = expected.id;
                let passenger = snapshot_vehicle_reference(&expected);
                let result = coordinator.remove_if_current(*expected);
                if matches!(&result, Ok(Some(_))) {
                    invalidate_selected_read_routes(
                        &selected_read_routes,
                        std::iter::once(entity).chain(passenger),
                    );
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::ReplaceSnapshotIfCurrent {
                expected,
                next,
                defer_journal,
                allow_type_change,
                reply,
            } => {
                let result = coordinator.replace_snapshot_if_current_inner(
                    *expected,
                    *next,
                    !defer_journal,
                    allow_type_change,
                );
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::PrepareSnapshot {
                expected,
                next,
                reply,
            } => {
                let _ = reply.send(coordinator.prepare_snapshot_if_current(*expected, *next));
            }
            RegionalOwnerCommand::CommitPreparedSnapshot {
                phase,
                lane,
                sequence,
                entity,
                reply,
            } => {
                let result = coordinator.commit_prepared_snapshot(phase, lane, sequence, entity);
                if result.is_ok() {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                } else {
                    coordinator
                        .commit_state
                        .outcome_unknown
                        .store(true, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::AbortPreparedSnapshot { phase, lane, reply } => {
                let result = coordinator.abort_prepared_snapshot(phase, lane);
                if result.is_err() {
                    coordinator
                        .commit_state
                        .outcome_unknown
                        .store(true, Ordering::Release);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::ReplaceSnapshotsIfCurrent { snapshots, reply } => {
                let result = coordinator.replace_snapshots_if_current(snapshots);
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::MergeItemSnapshotsIfCurrent {
                survivor_expected,
                survivor_next,
                consumed_expected,
                reply,
            } => {
                let result = coordinator.merge_item_snapshots_if_current(
                    *survivor_expected,
                    *survivor_next,
                    *consumed_expected,
                );
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::CommitVillagerBirthIfCurrent { commit, reply } => {
                let result = coordinator.commit_villager_birth_if_current(*commit);
                if matches!(&result, Ok(Some(_))) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::CommitVillagerInventoryPickupIfCurrent { commit, reply } => {
                let result = coordinator.commit_villager_inventory_pickup_if_current(*commit);
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::CommitVillagerFoodShareIfCurrent { commit, reply } => {
                let result = coordinator.commit_villager_food_share_if_current(*commit);
                if matches!(&result, Ok(Some(_))) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::CommitVillagerCourtshipIfCurrent { commit, reply } => {
                let result = coordinator.commit_villager_courtship_if_current(*commit);
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::CommitVillagerNoBedIfCurrent { commit, reply } => {
                let result = coordinator.commit_villager_no_bed_if_current(*commit);
                if matches!(&result, Ok(true)) {
                    write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                        .clear();
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SetAnimalStatesIfCurrent {
                states,
                defer_journal,
                reply,
            } => {
                let _ = reply
                    .send(coordinator.set_animal_states_if_current_inner(states, !defer_journal));
            }
            RegionalOwnerCommand::SetGoal {
                entity,
                goal,
                reply,
            } => {
                let _ = reply.send(coordinator.set_goal(entity, goal));
            }
            RegionalOwnerCommand::SetGoals {
                goals,
                defer_journal,
                reply,
            } => {
                let _ = reply.send(coordinator.set_goals_inner(goals, !defer_journal));
            }
            RegionalOwnerCommand::SetItemStack {
                entity,
                item_stack,
                reply,
            } => {
                let _ = reply.send(coordinator.set_item_stack(entity, item_stack));
            }
            RegionalOwnerCommand::SetItemStackIfCurrent {
                expected,
                item_stack,
                reply,
            } => {
                let _ = reply.send(coordinator.set_item_stack_if_current(*expected, item_stack));
            }
            RegionalOwnerCommand::ResolveItemPickupClaim {
                entity,
                claim,
                item_stack,
                defer_journal,
                reply,
            } => {
                let result = coordinator.resolve_item_pickup_claim_inner(
                    entity,
                    claim,
                    item_stack,
                    !defer_journal,
                );
                if matches!(&result, Ok(Some(_))) {
                    invalidate_selected_read_routes(&selected_read_routes, [entity]);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SetPosition {
                entity,
                position,
                reply,
            } => {
                let old_region = coordinator.locations.get(&entity).copied();
                let result = coordinator.set_position(entity, position);
                if matches!(&result, Ok(true)) && old_region != RegionKey::from_position(position) {
                    invalidate_selected_read_routes(&selected_read_routes, [entity]);
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::SetVelocities { velocities, reply } => {
                let _ = reply.send(coordinator.set_velocities(velocities));
            }
            RegionalOwnerCommand::ApplyKinematicsIfCurrent {
                states,
                defer_journal,
                reply,
            } => {
                let ids = states
                    .iter()
                    .map(|(expected, _)| expected.id)
                    .collect::<HashSet<_>>();
                let crosses_region = states.iter().any(|(expected, state)| {
                    RegionKey::from_position(expected.position)
                        != RegionKey::from_position(state.position)
                });
                let expected_count = ids.len();
                let mut publication_snapshots = states
                    .iter()
                    .map(|(expected, state)| snapshot_with_kinematics(expected.clone(), *state))
                    .collect::<Vec<_>>();
                publication_snapshots.sort_unstable_by_key(|snapshot| snapshot.id);
                let result = coordinator
                    .apply_kinematics_if_current_inner(states, !defer_journal)
                    .and_then(|applied| {
                        if !applied {
                            return Ok(VersionedEntitySnapshots {
                                authority: coordinator.authority,
                                fence: RegionalVersionFence {
                                    leases: HashMap::new(),
                                    lane_versions: BTreeMap::new(),
                                },
                                snapshots: Vec::new(),
                                publication_readers: None,
                            });
                        }
                        if crosses_region {
                            publication_snapshots = coordinator.snapshots_for_ids(&ids)?;
                            if publication_snapshots.len() != expected_count {
                                return Err(RegionOwnerLaneError::InvalidMutation);
                            }
                        }
                        let fence = selected_version_fence(&coordinator, &publication_snapshots)?;
                        Ok(VersionedEntitySnapshots {
                            authority: coordinator.authority,
                            fence,
                            snapshots: publication_snapshots,
                            publication_readers: None,
                        })
                    });
                if result
                    .as_ref()
                    .is_ok_and(|selected| selected.snapshots().len() == expected_count)
                {
                    if crosses_region {
                        write_benign_rwlock(&selected_read_routes, "regional.selected_read_routes")
                            .clear();
                    } else if let Ok(selected) = &result {
                        publish_selected_read_routes(
                            &coordinator,
                            &selected_read_routes,
                            &ids,
                            selected.snapshots(),
                        );
                    }
                }
                let _ = reply.send(result);
            }
            RegionalOwnerCommand::ApplyKinematicsIfVersioned {
                expected,
                states,
                reply,
            } => {
                let _ = reply.send(
                    coordinator.apply_kinematics_if_versioned_with_fence(expected, states, false),
                );
            }
            RegionalOwnerCommand::DamageIfCurrent {
                expected,
                request,
                reply,
            } => {
                let _ = reply.send(coordinator.damage_if_current(*expected, request));
            }
            RegionalOwnerCommand::ApplyEffectIfCurrent {
                expected,
                request,
                reply,
            } => {
                let _ = reply.send(coordinator.apply_effect_if_current(*expected, *request));
            }
            RegionalOwnerCommand::Damage {
                entity,
                request,
                reply,
            } => {
                let _ = reply.send(coordinator.damage(entity, request));
            }
            RegionalOwnerCommand::TickOwnedRegions { input, reply } => {
                let _ = reply.send(coordinator.tick_owned_regions(input));
            }
            RegionalOwnerCommand::CommitOwnedRegionPhysics { prepared, reply } => {
                let lane_exec_started = std::time::Instant::now();
                let mut output = coordinator.commit_owned_region_physics(*prepared);
                if let Ok(output) = output.as_mut() {
                    output.lane_timings.exec_us = lane_exec_started
                        .elapsed()
                        .as_micros()
                        .min(u128::from(u64::MAX))
                        as u64;
                }
                let _ = reply.send(output);
            }
            RegionalOwnerCommand::PrepareGoalTick {
                tick,
                active_ids,
                selected,
                inputs,
                reply,
            } => {
                #[cfg(test)]
                if let Some(probe) = goal_prepare_probe
                    .lock()
                    .expect("goal-prepare test probe lock poisoned")
                    .take()
                {
                    let _ = probe.send(());
                }
                let selected = selected
                    .filter(|selected| versioned_snapshots_are_current(&coordinator, selected))
                    .map(|selected| {
                        let lane_versions = selected.fence.lane_versions.clone();
                        (selected.into_snapshots(), lane_versions)
                    });
                let _ = reply.send(
                    coordinator.prepare_goal_tick_with_pathing_for_ids_from_snapshots(
                        tick,
                        &active_ids,
                        selected,
                        inputs,
                    ),
                );
            }
            RegionalOwnerCommand::ApplyPreparedGoalTick { resolved, reply } => {
                let _ = reply.send(coordinator.apply_prepared_goal_tick(*resolved));
            }
            RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
                resolved,
                entities,
                defer_journal,
                reply,
            } => {
                let result = coordinator.apply_prepared_goal_tick_and_kinematics_for_ids(
                    *resolved,
                    &entities,
                    !defer_journal,
                );
                drop(entities);
                // Goal batches never change entity routing. The selected-read
                // route remains valid; rebuilding or invalidating it would turn
                // an owner-local goal commit into a second 100k coordinator scan.
                let _ = reply.send(result);
            }

            #[cfg(test)]
            RegionalOwnerCommand::HoldForTest { entered, release } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
            #[cfg(test)]
            RegionalOwnerCommand::HoldLaneForTest {
                entity: _,
                entered,
                release,
            } => {
                let _ = entered.send(());
                let _ = release.recv();
            }
            #[cfg(test)]
            RegionalOwnerCommand::PanicForTest => {
                panic!("injected regional owner coordinator panic");
            }
            RegionalOwnerCommand::Shutdown { reply } => {
                let _ = reply.send(coordinator.shutdown());
                return;
            }
        }
    }
    let _ = coordinator.shutdown();
}

fn snapshot_kinematics(snapshot: EntitySnapshot) -> EntityKinematics {
    EntityKinematics {
        id: snapshot.id,
        position: snapshot.position,
        rotation: snapshot.rotation,
        velocity: snapshot.velocity,
        on_ground: snapshot.on_ground,
    }
}

fn snapshot_with_kinematics(
    mut snapshot: EntitySnapshot,
    state: EntityKinematics,
) -> EntitySnapshot {
    snapshot.position = state.position;
    snapshot.rotation = state.rotation;
    snapshot.velocity = state.velocity;
    snapshot.on_ground = state.on_ground;
    snapshot
}
fn selected_version_fence(
    coordinator: &RegionalOwnerCoordinator,
    snapshots: &[EntitySnapshot],
) -> Result<RegionalVersionFence, RegionOwnerLaneError> {
    let mut leases = HashMap::new();
    let mut versions = BTreeMap::new();
    for snapshot in snapshots {
        let key = coordinator
            .locations
            .get(&snapshot.id)
            .copied()
            .ok_or(RegionOwnerLaneError::UnknownEntity)?;
        let lease = coordinator
            .ownership
            .lease(key)
            .ok_or(RegionOwnerLaneError::UnknownRegion)?;
        let owner = coordinator
            .lanes
            .get(&lease.lane)
            .ok_or(RegionOwnerLaneError::WrongLane)?;
        if coordinator.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            || RegionKey::from_position(snapshot.position) != Some(key)
        {
            return Err(RegionOwnerLaneError::InvalidMutation);
        }
        leases.insert(snapshot.id, lease);
        versions
            .entry(lease.lane)
            .or_insert_with(|| owner.reader().state_version());
    }
    Ok(RegionalVersionFence {
        leases,
        lane_versions: versions,
    })
}

fn versioned_snapshots_are_current(
    coordinator: &RegionalOwnerCoordinator,
    selected: &VersionedEntitySnapshots,
) -> bool {
    let selected_lanes = selected
        .fence
        .leases
        .values()
        .map(|lease| lease.lane)
        .collect::<BTreeSet<_>>();
    selected.authority == coordinator.authority
        && selected.fence.leases.len() == selected.snapshots.len()
        && selected_lanes
            .iter()
            .eq(selected.fence.lane_versions.keys())
        && selected.snapshots.iter().all(|snapshot| {
            coordinator
                .locations
                .get(&snapshot.id)
                .and_then(|key| coordinator.ownership.lease(*key))
                .is_some_and(|lease| {
                    selected.fence.leases.get(&snapshot.id) == Some(&lease)
                        && coordinator.uuids.get(&snapshot.uuid).copied() == Some(snapshot.id)
                })
        })
        && selected.fence.lane_versions.iter().all(|(lane, version)| {
            coordinator
                .lanes
                .get(lane)
                .is_some_and(|owner| owner.reader().state_version() == *version)
        })
}

fn publish_selected_kinematics_routes(
    coordinator: &RegionalOwnerCoordinator,
    routes: &RwLock<HashMap<EntityId, CachedEntityReadRoute>>,
    requested: &HashSet<EntityId>,
    selected: &VersionedEntityKinematics,
) {
    if selected.expected.len() == requested.len()
        && selected.expected.len() == selected.routes.len()
    {
        let cached = read_benign_rwlock(routes, "regional.selected_read_routes");
        if selected
            .expected
            .iter()
            .zip(&selected.routes)
            .all(|(expected, lease)| {
                cached
                    .get(&expected.id)
                    .is_some_and(|route| route.uuid == expected.uuid && *lease == route.lease)
            })
        {
            return;
        }
    }
    let mut refreshed = Vec::with_capacity(selected.expected.len());
    for (expected, lease) in selected.expected.iter().zip(&selected.routes) {
        let Some(key) = coordinator.locations.get(&expected.id).copied() else {
            return;
        };
        let Some(owner) = coordinator.lanes.get(&lease.lane) else {
            return;
        };
        if coordinator.uuids.get(&expected.uuid).copied() != Some(expected.id)
            || RegionKey::from_position(expected.expected.position) != Some(key)
            || lease.key != key
        {
            return;
        }
        refreshed.push((
            expected.id,
            CachedEntityReadRoute {
                lease: *lease,
                uuid: expected.uuid,
                standalone: !coordinator.vehicle_passengers.contains_key(&expected.id)
                    && !coordinator.passenger_vehicles.contains_key(&expected.id),
                owner: owner.reader(),
            },
        ));
    }
    let mut routes = write_benign_rwlock(routes, "regional.selected_read_routes");
    for entity in requested {
        routes.remove(entity);
    }
    routes.extend(refreshed);
}

fn publish_selected_read_routes(
    coordinator: &RegionalOwnerCoordinator,
    routes: &RwLock<HashMap<EntityId, CachedEntityReadRoute>>,
    requested: &HashSet<EntityId>,
    snapshots: &[EntitySnapshot],
) {
    let mut refreshed = Vec::with_capacity(snapshots.len());
    for snapshot in snapshots {
        let Some(key) = coordinator.locations.get(&snapshot.id).copied() else {
            return;
        };
        let Some(lease) = coordinator.ownership.lease(key) else {
            return;
        };
        let Some(owner) = coordinator.lanes.get(&lease.lane) else {
            return;
        };
        if coordinator.uuids.get(&snapshot.uuid).copied() != Some(snapshot.id)
            || RegionKey::from_position(snapshot.position) != Some(key)
        {
            return;
        }
        refreshed.push((
            snapshot.id,
            CachedEntityReadRoute {
                lease,
                uuid: snapshot.uuid,
                standalone: !coordinator.vehicle_passengers.contains_key(&snapshot.id)
                    && !coordinator.passenger_vehicles.contains_key(&snapshot.id),
                owner: owner.reader(),
            },
        ));
    }

    let mut routes = write_benign_rwlock(routes, "regional.selected_read_routes");
    for entity in requested {
        routes.remove(entity);
    }
    routes.extend(refreshed);
}

fn invalidate_selected_read_routes(
    routes: &RwLock<HashMap<EntityId, CachedEntityReadRoute>>,
    entities: impl IntoIterator<Item = EntityId>,
) {
    let mut routes = write_benign_rwlock(routes, "regional.selected_read_routes");
    for entity in entities {
        routes.remove(&entity);
    }
}

fn recover_owner_runtime_start(
    coordinator: RegionalOwnerCoordinator,
    error: RegionOwnerLaneError,
) -> RegionalOwnerCutoverError {
    match coordinator.shutdown() {
        Ok(store) => RegionalOwnerCutoverError {
            error,
            store: Box::new(store),
        },
        Err(shutdown) => RegionalOwnerCutoverError {
            error: shutdown.error,
            store: shutdown.recovered,
        },
    }
}

enum OwnerKinematicsPlan {
    Local {
        source: RegionKey,
        expected: Box<EntitySnapshot>,
        state: EntityKinematics,
    },
    Migrate {
        source: RegionKey,
        target: RegionKey,
        expected: Vec<EntitySnapshot>,
        moved: Vec<EntitySnapshot>,
    },
}

fn journal_entities_by_lane(
    mutations: &BTreeMap<usize, Vec<SequencedRegionMutation>>,
) -> BTreeMap<usize, Vec<(RegionLease, EntityId)>> {
    let mut entities = BTreeMap::<usize, BTreeMap<EntityId, RegionLease>>::new();
    for (&lane, mutations) in mutations {
        for mutation in mutations {
            for entity in mutation_entity_ids(&mutation.mutation) {
                entities
                    .entry(lane)
                    .or_default()
                    .insert(entity, mutation.lease);
            }
        }
    }
    entities
        .into_iter()
        .map(|(lane, entities)| {
            (
                lane,
                entities
                    .into_iter()
                    .map(|(entity, lease)| (lease, entity))
                    .collect(),
            )
        })
        .collect()
}

fn mutation_entity_ids(mutation: &RegionOwnerMutation) -> Vec<EntityId> {
    match mutation {
        RegionOwnerMutation::SetVelocity { entity, .. }
        | RegionOwnerMutation::SetAnimalState { entity, .. }
        | RegionOwnerMutation::RemoveEntity(entity) => vec![*entity],
        RegionOwnerMutation::SetAnimalStateIfCurrent { expected, .. }
        | RegionOwnerMutation::SetGrazingStateIfCurrent { expected, .. }
        | RegionOwnerMutation::SetGoalIfCurrent { expected, .. }
        | RegionOwnerMutation::SetItemStackIfCurrent { expected, .. }
        | RegionOwnerMutation::ReplaceSnapshotIfCurrent { expected, .. }
        | RegionOwnerMutation::SetKinematicsIfCurrent { expected, .. }
        | RegionOwnerMutation::DamageIfCurrent { expected, .. }
        | RegionOwnerMutation::ApplyEffectIfCurrent { expected, .. }
        | RegionOwnerMutation::RemoveIfCurrent(expected)
        | RegionOwnerMutation::InsertSnapshot(expected) => vec![expected.id],
        RegionOwnerMutation::ApplyGoalBatch { expected, .. } => {
            expected.iter().map(|checkpoint| checkpoint.id).collect()
        }
        RegionOwnerMutation::SetKinematicsBatchIfCurrent { expected, .. }
        | RegionOwnerMutation::InsertSnapshots(expected)
        | RegionOwnerMutation::RemoveSnapshotsIfCurrent(expected) => {
            expected.iter().map(|snapshot| snapshot.id).collect()
        }
        RegionOwnerMutation::SetKinematicsBatchIfVersion { previous, .. } => {
            previous.iter().map(|entry| entry.id).collect()
        }
    }
}

fn retain_recovered_regional_state(
    ownership: &mut RegionOwnership,
    stores: &BTreeMap<RegionKey, EntityStore>,
    locations: &mut BTreeMap<EntityId, RegionKey>,
    uuids: &mut HashMap<Uuid, EntityId>,
) {
    ownership.owners.retain(|key, _| stores.contains_key(key));
    ownership.active_phase = None;
    ownership.pending_lanes.clear();
    locations.retain(|entity, key| stores.get(key).is_some_and(|store| store.contains(*entity)));
    uuids.retain(|_, entity| locations.contains_key(entity));
}

#[derive(Debug)]
pub struct RegionalEntityAuthority {
    regions: RegionalEntityStore,
    lane: usize,
    transfer_tick: u64,
}

struct SingleLanePhase<'a> {
    regions: &'a mut RegionalEntityStore,
    phase: RegionPhase,
    lane: usize,
    finished: bool,
}

impl SingleLanePhase<'_> {
    fn finish(mut self) -> Result<(), RegionEntityStoreError> {
        if !self.regions.stores.is_empty() {
            self.regions.acknowledge_lane(self.phase, self.lane)?;
        }
        self.regions.finish_phase(self.phase)?;
        self.finished = true;
        Ok(())
    }
}

impl Drop for SingleLanePhase<'_> {
    fn drop(&mut self) {
        if self.finished {
            return;
        }
        if !self.regions.stores.is_empty() {
            let _ = self.regions.acknowledge_lane(self.phase, self.lane);
        }
        let _ = self.regions.finish_phase(self.phase);
    }
}

impl RegionalEntityAuthority {
    #[must_use]
    pub fn with_next_id(next_id: i32) -> Self {
        let mut regions = RegionalEntityStore::new();
        regions.next_id = next_id.max(0);
        Self {
            regions,
            lane: 0,
            transfer_tick: 0,
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.regions.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    #[must_use]
    pub fn contains(&self, id: EntityId) -> bool {
        self.regions.contains(id)
    }

    #[must_use]
    pub fn contains_uuid(&self, uuid: Uuid) -> bool {
        self.regions.contains_uuid(uuid)
    }

    #[must_use]
    pub fn snapshot(&self, id: EntityId) -> Option<EntitySnapshot> {
        self.regions.snapshot(id)
    }

    #[must_use]
    pub fn motion_state(&self, id: EntityId) -> Option<EntityMotionState> {
        self.regions.motion_state(id)
    }

    pub fn snapshots(&self) -> impl Iterator<Item = EntitySnapshot> + '_ {
        self.regions.snapshots()
    }

    #[must_use]
    pub fn region_len(&self, key: RegionKey) -> usize {
        self.regions.region_len(key)
    }

    #[must_use]
    pub fn parallel_kinematics_batch_count(&self, states: &[EntityKinematics]) -> usize {
        if states.len() < PARALLEL_KINEMATICS_MIN_STATES {
            return 0;
        }
        let mut ids = HashSet::with_capacity(states.len());
        if states.iter().any(|state| !ids.insert(state.id)) {
            return 0;
        }
        states
            .iter()
            .filter_map(|state| {
                if !state.is_finite() {
                    return None;
                }
                let source = self.regions.region_for(state.id)?;
                (RegionKey::from_position(state.position) == Some(source)).then_some(source)
            })
            .collect::<BTreeSet<_>>()
            .len()
    }

    pub fn spawn(&mut self, entity: SpawnEntity) -> EntityId {
        let key = self.ensure_position_region(entity.position);
        self.with_phase(|regions, phase| {
            let lease = regions.ownership.lease(key).expect("assigned spawn region");
            regions
                .spawn(phase, lease, entity)
                .expect("validated production entity spawn")
        })
    }

    pub fn spawn_batch(
        &mut self,
        entities: impl IntoIterator<Item = SpawnEntity>,
    ) -> Vec<EntityId> {
        let entities = entities.into_iter().collect::<Vec<_>>();
        for entity in &entities {
            self.ensure_position_region(entity.position);
        }
        self.with_phase(|regions, phase| {
            regions
                .spawn_batch(phase, entities)
                .expect("validated production entity batch")
        })
    }

    pub fn insert_snapshot(&mut self, snapshot: EntitySnapshot) -> bool {
        self.ensure_position_region(snapshot.position);
        self.with_phase(|regions, phase| regions.insert_snapshots(phase, [snapshot]).is_ok())
    }

    pub fn insert_snapshots_batch(
        &mut self,
        snapshots: impl IntoIterator<Item = EntitySnapshot>,
    ) -> bool {
        let snapshots = snapshots.into_iter().collect::<Vec<_>>();
        for snapshot in &snapshots {
            self.ensure_position_region(snapshot.position);
        }
        let expected = snapshots.len();
        self.with_phase(|regions, phase| regions.insert_snapshots(phase, snapshots) == Ok(expected))
    }

    pub fn set_animal_state(&mut self, id: EntityId, animal: AnimalBreedingState) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_animal_state(phase, id, animal).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_animal_states(
        &mut self,
        states: impl IntoIterator<Item = (EntityId, AnimalBreedingState)>,
    ) -> usize {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return 0;
        }
        self.with_phase(|regions, phase| {
            states
                .into_iter()
                .filter(|(id, animal)| {
                    regions
                        .set_animal_state(phase, *id, *animal)
                        .unwrap_or(false)
                })
                .count()
        })
    }

    pub fn set_animal_states_if_current(
        &mut self,
        states: impl IntoIterator<Item = (EntitySnapshot, AnimalBreedingState)>,
    ) -> bool {
        let states = states.into_iter().collect::<Vec<_>>();
        if states.is_empty() {
            return true;
        }
        let mut entity_ids = HashSet::with_capacity(states.len());
        if states.iter().any(|(expected, _)| {
            expected.animal.is_none()
                || !entity_ids.insert(expected.id)
                || self.regions.in_flight_transfers.contains_key(&expected.id)
                || self.regions.snapshot(expected.id).as_ref() != Some(expected)
        }) {
            return false;
        }

        self.with_phase(|regions, phase| {
            states.into_iter().all(|(expected, animal)| {
                regions
                    .set_animal_state(phase, expected.id, animal)
                    .unwrap_or(false)
            })
        })
    }

    pub fn set_goal(&mut self, id: EntityId, goal: GoalState) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_goal(phase, id, goal).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_goals(&mut self, goals: impl IntoIterator<Item = (EntityId, GoalState)>) -> usize {
        let goals = goals.into_iter().collect::<Vec<_>>();
        if goals.is_empty() {
            return 0;
        }
        self.with_phase(|regions, phase| {
            goals
                .into_iter()
                .filter(|(id, goal)| regions.set_goal(phase, *id, goal.clone()).unwrap_or(false))
                .count()
        })
    }

    pub fn set_item_stack(&mut self, id: EntityId, stack: Option<EntityItemStack>) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions
                .store_for_entity_mutation(phase, id)
                .is_ok_and(|store| store.set_item_stack(id, stack))
        })
        .unwrap_or(false)
    }

    pub fn set_velocity(&mut self, id: EntityId, velocity: Vec3) -> bool {
        self.with_entity_phase(id, |regions, phase| {
            regions.set_velocity(phase, id, velocity).unwrap_or(false)
        })
        .unwrap_or(false)
    }

    pub fn set_position(&mut self, id: EntityId, position: Vec3) -> bool {
        let Some(mut motion) = self.motion_state(id) else {
            return false;
        };
        if !position.is_finite() {
            return false;
        }
        self.ensure_position_region(position);
        motion.position = position;
        self.apply_kinematics([EntityKinematics {
            id,
            position: motion.position,
            rotation: motion.rotation,
            velocity: motion.velocity,
            on_ground: motion.on_ground,
        }]) == 1
    }

    pub fn damage(&mut self, id: EntityId, request: EntityDamageRequest) -> Option<EntityDamage> {
        self.with_entity_phase(id, |regions, phase| {
            regions.damage(phase, id, request).ok().flatten()
        })
        .flatten()
    }

    pub fn remove(&mut self, id: EntityId) -> Option<EntitySnapshot> {
        let key = self.regions.region_for(id)?;
        self.with_phase(|regions, phase| {
            let lease = regions.ownership.lease(key).expect("known entity region");
            regions.remove(phase, lease, id).ok()
        })
    }

    pub fn apply_kinematics(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> usize {
        self.apply_kinematics_serial(states.into_iter().collect())
            .len()
    }

    pub fn apply_kinematics_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
    ) -> Vec<EntityKinematics> {
        let applied = self.apply_kinematics_serial(states.into_iter().collect());
        self.authoritative_kinematics(applied)
    }

    pub fn apply_kinematics_parallel(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
    ) -> usize {
        self.apply_kinematics_parallel_inner(states.into_iter().collect(), max_workers, &|_| {})
            .len()
    }

    pub fn apply_kinematics_parallel_authoritative(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
    ) -> Vec<EntityKinematics> {
        let applied = self.apply_kinematics_parallel_inner(
            states.into_iter().collect(),
            max_workers,
            &|_| {},
        );
        self.authoritative_kinematics(applied)
    }

    #[cfg(test)]
    fn apply_kinematics_parallel_with_probe(
        &mut self,
        states: impl IntoIterator<Item = EntityKinematics>,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> usize {
        self.apply_kinematics_parallel_inner(
            states.into_iter().collect(),
            max_workers,
            before_region,
        )
        .len()
    }

    fn apply_kinematics_parallel_inner(
        &mut self,
        states: Vec<EntityKinematics>,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> Vec<EntityId> {
        if max_workers <= 1 {
            return self.apply_kinematics_serial(states);
        }
        let states = states
            .into_iter()
            .filter(|state| state.is_finite())
            .collect::<Vec<_>>();
        let mut unique_ids = HashSet::with_capacity(states.len());
        if states.iter().any(|state| !unique_ids.insert(state.id)) {
            return self.apply_kinematics_serial(states);
        }
        for state in &states {
            self.ensure_position_region(state.position);
        }

        let mut crossing_leaders = HashMap::new();
        for state in &states {
            let Some(source) = self.regions.region_for(state.id) else {
                continue;
            };
            if RegionKey::from_position(state.position) == Some(source) {
                continue;
            }
            if let Ok(group) = self.regions.vehicle_group_snapshots(source, state.id)
                && let Some(leader) = vehicle_group_leader(&group)
            {
                for member in group {
                    crossing_leaders.insert(member.id, leader);
                }
            }
        }

        let mut ordered = states;
        ordered.sort_by_key(|state| {
            let leader = crossing_leaders.get(&state.id).copied().unwrap_or(state.id);
            (leader, state.id != leader)
        });
        let serial_fallback = ordered.clone();
        let mut local_by_region = BTreeMap::<RegionKey, Vec<EntityKinematics>>::new();
        let mut crossings = Vec::new();
        for state in ordered {
            let Some(source) = self.regions.region_for(state.id) else {
                crossings.push(state);
                continue;
            };
            let target = RegionKey::from_position(state.position);
            if crossing_leaders.contains_key(&state.id) || target != Some(source) {
                crossings.push(state);
            } else {
                local_by_region.entry(source).or_default().push(state);
            }
        }
        let worker_count = max_workers.min(local_by_region.len());
        if worker_count <= 1 {
            return self.apply_kinematics_serial(serial_fallback);
        }

        self.transfer_tick = self
            .transfer_tick
            .checked_add(1)
            .expect("production transfer tick exhausted");
        let transfer_tick = self.transfer_tick;
        self.with_phase(|regions, phase| {
            let applied = std::sync::Mutex::new(Vec::new());
            {
                let mut batches = regions
                    .stores
                    .iter_mut()
                    .filter_map(|(&key, store)| {
                        local_by_region
                            .remove(&key)
                            .map(|states| (key, store, states))
                    })
                    .collect::<Vec<_>>();
                let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
                for (index, batch) in batches.drain(..).enumerate() {
                    buckets[index % worker_count].push(batch);
                }
                let local = buckets.pop().expect("positive regional worker count");
                rayon::scope(|scope| {
                    for bucket in buckets {
                        let applied = &applied;
                        scope.spawn(move |_| {
                            for (key, store, states) in bucket {
                                before_region(key);
                                let accepted = apply_local_kinematics(store, states);
                                lock_authoritative_mutex(
                                    applied,
                                    "regional.kinematics_parallel_results",
                                )
                                .extend(accepted);
                            }
                        });
                    }
                    for (key, store, states) in local {
                        before_region(key);
                        let accepted = apply_local_kinematics(store, states);
                        lock_authoritative_mutex(&applied, "regional.kinematics_parallel_results")
                            .extend(accepted);
                    }
                });
            }
            let mut applied = applied
                .into_inner()
                .expect("regional kinematics parallel result lock poisoned");
            applied.extend(apply_kinematics_in_phase(
                regions,
                phase,
                transfer_tick,
                crossings,
            ));
            applied.sort_unstable();
            applied
        })
    }

    fn apply_kinematics_serial(&mut self, states: Vec<EntityKinematics>) -> Vec<EntityId> {
        let mut states = states
            .into_iter()
            .filter(|state| state.is_finite())
            .collect::<Vec<_>>();
        if states.is_empty() {
            return Vec::new();
        }
        for state in &states {
            self.ensure_position_region(state.position);
        }
        let mut crossing_leaders = HashMap::new();
        for state in &states {
            let Some(source) = self.regions.region_for(state.id) else {
                continue;
            };
            if RegionKey::from_position(state.position) == Some(source) {
                continue;
            }
            if let Ok(group) = self.regions.vehicle_group_snapshots(source, state.id)
                && let Some(leader) = vehicle_group_leader(&group)
            {
                for member in group {
                    crossing_leaders.insert(member.id, leader);
                }
            }
        }
        states.sort_by_key(|state| {
            let leader = crossing_leaders.get(&state.id).copied().unwrap_or(state.id);
            (leader, state.id != leader)
        });
        self.transfer_tick = self
            .transfer_tick
            .checked_add(1)
            .expect("production transfer tick exhausted");
        let transfer_tick = self.transfer_tick;
        self.with_phase(|regions, phase| {
            apply_kinematics_in_phase(regions, phase, transfer_tick, states)
        })
    }

    fn authoritative_kinematics(&self, ids: Vec<EntityId>) -> Vec<EntityKinematics> {
        ids.into_iter()
            .filter_map(|id| {
                self.motion_state(id).map(|state| EntityKinematics {
                    id,
                    position: state.position,
                    rotation: state.rotation,
                    velocity: state.velocity,
                    on_ground: state.on_ground,
                })
            })
            .collect()
    }

    pub fn alive_kinematics_for_ids(&mut self, ids: &HashSet<EntityId>) -> Vec<EntityKinematics> {
        let mut states = self
            .regions
            .stores
            .values_mut()
            .flat_map(|store| store.alive_kinematics_for_ids(ids))
            .collect::<Vec<_>>();
        states.sort_unstable_by_key(|state| state.id);
        states
    }

    pub fn prepare_goal_tick_with_pathing_for_ids(
        &mut self,
        tick: u64,
        active_ids: &HashSet<EntityId>,
    ) -> RegionalPreparedGoalTick {
        let mut ids_by_region = BTreeMap::<RegionKey, HashSet<EntityId>>::new();
        for id in active_ids {
            if let Some(key) = self.regions.region_for(*id) {
                ids_by_region.entry(key).or_default().insert(*id);
            }
        }
        let simulation_ids = ids_by_region
            .iter()
            .map(|(&key, ids)| {
                let mut ids = ids.iter().copied().collect::<Vec<_>>();
                ids.sort_unstable();
                (key, ids)
            })
            .collect();
        let captured = self.regions.capture_follow_targets(&ids_by_region);
        let batches: BTreeMap<RegionKey, PreparedGoalTick> = ids_by_region
            .into_iter()
            .map(|(key, ids)| {
                let prepared = self
                    .regions
                    .stores
                    .get_mut(&key)
                    .expect("indexed entity region")
                    .prepare_goal_tick_with_pathing_for_ids(tick, &ids);
                (key, prepared)
            })
            .collect();
        let lease_keys = batches
            .keys()
            .copied()
            .chain(captured.sources.values().map(|source| source.region))
            .collect::<BTreeSet<_>>();
        RegionalPreparedGoalTick {
            authority: self.regions.authority,
            phase: RegionPhase(0),
            leases: lease_keys
                .into_iter()
                .map(|key| {
                    (
                        key,
                        self.regions
                            .ownership
                            .lease(key)
                            .expect("production regional goal lease"),
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
        }
    }

    pub fn apply_prepared_goal_tick(
        &mut self,
        resolved: RegionalResolvedGoalTick,
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, 1, &|_| {})
    }

    pub fn apply_prepared_goal_tick_parallel(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        max_workers: usize,
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, max_workers, &|_| {})
    }

    #[cfg(test)]
    fn apply_prepared_goal_tick_parallel_with_probe(
        &mut self,
        resolved: RegionalResolvedGoalTick,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> GoalTickStats {
        self.apply_prepared_goal_tick_parallel_inner(resolved, max_workers, before_region)
    }

    fn apply_prepared_goal_tick_parallel_inner(
        &mut self,
        mut resolved: RegionalResolvedGoalTick,
        max_workers: usize,
        before_region: &(dyn Fn(RegionKey) + Sync),
    ) -> GoalTickStats {
        if resolved.authority != self.regions.authority {
            return GoalTickStats::default();
        }
        if resolved
            .leases
            .iter()
            .any(|(key, lease)| lease.key != *key || !self.regions.ownership.validate(*lease))
        {
            return GoalTickStats::default();
        }
        if resolved
            .expected_missing_follow_targets
            .iter()
            .any(|target| self.regions.region_for(*target).is_some())
        {
            return GoalTickStats::default();
        }
        if !self
            .regions
            .follow_target_sources_match(&resolved.follow_target_sources)
        {
            return GoalTickStats::default();
        }
        if !self.regions.goal_inputs_match(&resolved.goal_inputs) {
            return GoalTickStats::default();
        }
        let worker_count = max_workers.max(1).min(resolved.batches.len().max(1));
        if worker_count == 1 {
            let mut total = GoalTickStats::default();
            let mut follow_targets = resolved.follow_targets;
            for (key, batch) in resolved.batches {
                if let Some(store) = self.regions.stores.get_mut(&key) {
                    let targets = follow_targets.remove(&key).unwrap_or_default();
                    add_goal_tick_stats(
                        &mut total,
                        store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                    );
                }
            }
            return total;
        }

        let mut follow_targets = resolved.follow_targets;
        let mut batches = self
            .regions
            .stores
            .iter_mut()
            .filter_map(|(&key, store)| {
                let batch = resolved.batches.remove(&key)?;
                let targets = follow_targets.remove(&key).unwrap_or_default();
                Some((key, store, batch, targets))
            })
            .collect::<Vec<_>>();
        let mut buckets = (0..worker_count).map(|_| Vec::new()).collect::<Vec<_>>();
        for (index, batch) in batches.drain(..).enumerate() {
            buckets[index % worker_count].push(batch);
        }
        let local = buckets.pop().expect("positive regional worker count");
        let total = std::sync::Mutex::new(GoalTickStats::default());
        rayon::scope(|scope| {
            for bucket in buckets {
                let total = &total;
                scope.spawn(move |_| {
                    let mut worker_total = GoalTickStats::default();
                    for (key, store, batch, targets) in bucket {
                        before_region(key);
                        add_goal_tick_stats(
                            &mut worker_total,
                            store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                        );
                    }
                    add_goal_tick_stats(
                        &mut lock_authoritative_mutex(total, "regional.goal_apply_parallel_stats"),
                        worker_total,
                    );
                });
            }
            let mut local_total = GoalTickStats::default();
            for (key, store, batch, targets) in local {
                before_region(key);
                add_goal_tick_stats(
                    &mut local_total,
                    store.apply_prepared_goal_tick_with_follow_targets(batch, &targets),
                );
            }
            add_goal_tick_stats(
                &mut lock_authoritative_mutex(&total, "regional.goal_apply_parallel_stats"),
                local_total,
            );
        });
        total
            .into_inner()
            .expect("regional goal parallel stats lock poisoned")
    }

    pub fn visit_simulation_entities(&self, visitor: impl FnMut(EntityView<'_>)) {
        self.regions.visit_simulation_entities(visitor);
    }

    pub fn visit_simulation_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        visitor: impl FnMut(EntityView<'_>),
    ) {
        self.regions.visit_simulation_entities_for_ids(ids, visitor);
    }

    pub fn visit_sheep_entities_for_ids(
        &self,
        ids: &HashSet<EntityId>,
        visitor: impl FnMut(EntityView<'_>),
    ) {
        self.regions.visit_sheep_entities_for_ids(ids, visitor);
    }

    fn ensure_position_region(&mut self, position: Vec3) -> RegionKey {
        let key = RegionKey::from_position(position).expect("finite production entity position");
        if self.regions.ownership.lease(key).is_none() {
            self.regions
                .assign_region(key, self.lane)
                .expect("inactive production region assignment");
        }
        key
    }

    fn with_entity_phase<R>(
        &mut self,
        id: EntityId,
        operation: impl FnOnce(&mut RegionalEntityStore, RegionPhase) -> R,
    ) -> Option<R> {
        self.regions.region_for(id)?;
        Some(self.with_phase(operation))
    }

    fn with_phase<R>(
        &mut self,
        operation: impl FnOnce(&mut RegionalEntityStore, RegionPhase) -> R,
    ) -> R {
        let phase = self.regions.begin_phase().expect("production region phase");
        let phase_scope = SingleLanePhase {
            regions: &mut self.regions,
            phase,
            lane: self.lane,
            finished: false,
        };
        let result = operation(phase_scope.regions, phase);
        phase_scope
            .finish()
            .expect("single production lane phase completion");
        result
    }
}

impl Default for RegionalEntityAuthority {
    fn default() -> Self {
        Self::with_next_id(0)
    }
}

fn apply_kinematics_in_phase(
    regions: &mut RegionalEntityStore,
    phase: RegionPhase,
    transfer_tick: u64,
    states: impl IntoIterator<Item = EntityKinematics>,
) -> Vec<EntityId> {
    let mut applied = Vec::new();
    let mut transferred = HashSet::new();
    for state in states {
        if transferred.contains(&state.id) {
            applied.push(state.id);
            continue;
        }
        match regions.apply_kinematics(phase, transfer_tick, state) {
            Ok(RegionalKinematicsApply::AppliedLocal) => applied.push(state.id),
            Ok(RegionalKinematicsApply::PreparedTransfer(transfer)) => {
                let group = regions.transfer_entity_ids(transfer);
                if regions
                    .decide_transfer(phase, transfer, TransferDecision::Commit)
                    .and_then(|()| regions.apply_transfer(phase, transfer))
                    .is_ok()
                {
                    transferred.extend(group);
                    applied.push(state.id);
                }
            }
            Err(_) => {}
        }
    }
    applied
}

fn apply_local_kinematics(store: &mut EntityStore, states: Vec<EntityKinematics>) -> Vec<EntityId> {
    let expected = states.clone();
    let _ = store.apply_kinematics(states);
    expected
        .into_iter()
        .filter_map(|expected| {
            store
                .motion_state(expected.id)
                .is_some_and(|actual| {
                    actual.position == expected.position
                        && actual.rotation == expected.rotation
                        && actual.velocity == expected.velocity
                        && actual.on_ground == expected.on_ground
                })
                .then_some(expected.id)
        })
        .collect()
}

impl PreparedTransfer {
    fn matches_request(
        &self,
        phase: RegionPhase,
        source: RegionLease,
        target: RegionLease,
        state: EntityKinematics,
    ) -> bool {
        self.phase == phase
            && self.source == source
            && self.target == target
            && self
                .target_snapshots
                .iter()
                .find(|snapshot| snapshot.id == state.id)
                .is_some_and(|snapshot| {
                    snapshot.position == state.position
                        && snapshot.rotation == state.rotation
                        && snapshot.velocity == state.velocity
                        && snapshot.on_ground == state.on_ground
                })
    }
}

fn entity_vehicle_reference(entity: &SpawnEntity) -> Option<EntityId> {
    entity.vehicle.and_then(|vehicle| vehicle.passenger)
}

fn snapshot_vehicle_reference(entity: &EntitySnapshot) -> Option<EntityId> {
    entity.vehicle.and_then(|vehicle| vehicle.passenger)
}

fn vehicle_group_leader(group: &[EntitySnapshot]) -> Option<EntityId> {
    let passengers = group
        .iter()
        .filter_map(snapshot_vehicle_reference)
        .collect::<HashSet<_>>();
    group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !passengers.contains(id))
        .min()
}

fn order_vehicle_group_for_removal(group: &[EntitySnapshot]) -> Vec<EntityId> {
    let by_id = group
        .iter()
        .map(|snapshot| (snapshot.id, snapshot))
        .collect::<HashMap<_, _>>();
    let passengers = group
        .iter()
        .filter_map(snapshot_vehicle_reference)
        .collect::<HashSet<_>>();
    let mut leaders = group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !passengers.contains(id))
        .collect::<Vec<_>>();
    leaders.sort_unstable();
    let mut ordered = Vec::with_capacity(group.len());
    let mut visited = HashSet::with_capacity(group.len());
    for leader in leaders {
        let mut current = Some(leader);
        while let Some(entity) = current {
            if !visited.insert(entity) {
                break;
            }
            ordered.push(entity);
            current = by_id
                .get(&entity)
                .and_then(|snapshot| snapshot_vehicle_reference(snapshot));
        }
    }
    let mut remaining = group
        .iter()
        .map(|snapshot| snapshot.id)
        .filter(|id| !visited.contains(id))
        .collect::<Vec<_>>();
    remaining.sort_unstable();
    ordered.extend(remaining);
    ordered
}

fn goal_checkpoint_from_snapshot(snapshot: EntitySnapshot) -> EntityGoalCheckpoint {
    EntityGoalCheckpoint {
        id: snapshot.id,
        position: snapshot.position,
        rotation: snapshot.rotation,
        velocity: snapshot.velocity,
        on_ground: snapshot.on_ground,
        lifecycle: snapshot.lifecycle,
        goal: snapshot.goal,
        path: snapshot.retained.path,
    }
}

fn goal_reference(goal: &GoalState) -> Option<EntityId> {
    match goal {
        GoalState::FollowTarget { target, .. } => Some(*target),
        _ => None,
    }
}

fn add_goal_tick_stats(total: &mut GoalTickStats, batch: GoalTickStats) {
    total.alive_entities += batch.alive_entities;
    total.decisions_applied += batch.decisions_applied;
    total.skipped_non_alive += batch.skipped_non_alive;
    total.missing_follow_targets += batch.missing_follow_targets;
    total.pathing_moves += batch.pathing_moves;
    total.pathing_blocked += batch.pathing_blocked;
    total.pathing_unloaded += batch.pathing_unloaded;
}

#[cfg(test)]
mod tests;
