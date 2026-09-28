use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use super::{
    RegionEntityStoreError, RegionKey, RegionOwnershipError, RegionalEntityAuthority,
    RegionalEntityStore, TransferApply, TransferDecision,
};
use crate::effects_26_1_2::{
    ActiveEffectChainSnapshot, ActiveEffectsSnapshot, EffectFlags, EffectId, EffectInstance,
    EffectKind,
};
use crate::{
    AnimalBreedingState, EntityActiveEffectsState, EntityDamageRequest, EntityId, EntityItemStack,
    EntityKinematics, EntitySnapshot, EntityStore, GoalState, GoalTickStats, PathingBudget,
    PathingProbe, PathingProbeResult, Rotation, SheepColor, SpawnEntity, Vec3, VehicleKind,
    VehicleState,
};
use uuid::Uuid;

fn damage_request(amount: f32) -> EntityDamageRequest {
    EntityDamageRequest {
        amount,
        tick: 10,
        death_remove_tick: 30,
        villager_gossip_event: None,
    }
}

// Combat target density is far higher than chunk-publication density; a
// four-block cell keeps exact nearest-neighbour buckets small without
// sampling or capping candidates.
const BATTLE_SPATIAL_CELL_SIZE: f64 = 4.0;

struct BattleSpatialGrid {
    cells: HashMap<(i32, i32), Vec<EntityId>>,
    bounds: Option<(i32, i32, i32, i32)>,
}

impl BattleSpatialGrid {
    fn from_ids(
        ids: impl IntoIterator<Item = EntityId>,
        positions: &HashMap<EntityId, Vec3>,
    ) -> Self {
        let mut cells = HashMap::<(i32, i32), Vec<EntityId>>::new();
        let mut bounds: Option<(i32, i32, i32, i32)> = None;
        for id in ids {
            let position = positions
                .get(&id)
                .expect("battle spatial entity must have a position");
            let cell = battle_spatial_cell(*position);
            cells.entry(cell).or_default().push(id);
            bounds = Some(match bounds {
                None => (cell.0, cell.0, cell.1, cell.1),
                Some((min_x, max_x, min_z, max_z)) => (
                    min_x.min(cell.0),
                    max_x.max(cell.0),
                    min_z.min(cell.1),
                    max_z.max(cell.1),
                ),
            });
        }
        Self { cells, bounds }
    }

    fn choose_target(
        &self,
        actor: EntityId,
        positions: &HashMap<EntityId, Vec3>,
    ) -> Option<EntityId> {
        let actor_position = *positions.get(&actor)?;
        let actor_cell = battle_spatial_cell(actor_position);
        let (min_x, max_x, min_z, max_z) = self.bounds?;
        let max_ring = i64::from(actor_cell.0)
            .abs_diff(i64::from(min_x))
            .max(i64::from(actor_cell.0).abs_diff(i64::from(max_x)))
            .max(i64::from(actor_cell.1).abs_diff(i64::from(min_z)))
            .max(i64::from(actor_cell.1).abs_diff(i64::from(max_z)));
        let mut best = None::<(f64, EntityId)>;

        for ring in 0..=max_ring {
            let ring = i32::try_from(ring).expect("battle spatial ring fits i32");
            let low_x = actor_cell.0 - ring;
            let high_x = actor_cell.0 + ring;
            let low_z = actor_cell.1 - ring;
            let high_z = actor_cell.1 + ring;
            for cell_x in low_x..=high_x {
                battle_spatial_consider_cell(
                    &self.cells,
                    (cell_x, low_z),
                    actor,
                    actor_position,
                    positions,
                    &mut best,
                );
                if high_z != low_z {
                    battle_spatial_consider_cell(
                        &self.cells,
                        (cell_x, high_z),
                        actor,
                        actor_position,
                        positions,
                        &mut best,
                    );
                }
            }
            for cell_z in (low_z + 1)..high_z {
                battle_spatial_consider_cell(
                    &self.cells,
                    (low_x, cell_z),
                    actor,
                    actor_position,
                    positions,
                    &mut best,
                );
                if high_x != low_x {
                    battle_spatial_consider_cell(
                        &self.cells,
                        (high_x, cell_z),
                        actor,
                        actor_position,
                        positions,
                        &mut best,
                    );
                }
            }

            if let Some((best_distance_squared, _)) = best {
                let covered_min_x = f64::from(low_x) * BATTLE_SPATIAL_CELL_SIZE;
                let covered_max_x = f64::from(high_x + 1) * BATTLE_SPATIAL_CELL_SIZE;
                let covered_min_z = f64::from(low_z) * BATTLE_SPATIAL_CELL_SIZE;
                let covered_max_z = f64::from(high_z + 1) * BATTLE_SPATIAL_CELL_SIZE;
                let unvisited_distance = (actor_position.x - covered_min_x)
                    .min(covered_max_x - actor_position.x)
                    .min(actor_position.z - covered_min_z)
                    .min(covered_max_z - actor_position.z);
                // Strict inequality is required: an unvisited candidate at
                // exactly the lower bound could win the EntityId tie-break.
                if best_distance_squared < unvisited_distance * unvisited_distance {
                    return best.map(|(_, id)| id);
                }
            }
        }
        best.map(|(_, id)| id)
    }
}

fn battle_spatial_cell(position: Vec3) -> (i32, i32) {
    assert!(position.x.is_finite() && position.z.is_finite());
    (
        (position.x / BATTLE_SPATIAL_CELL_SIZE).floor() as i32,
        (position.z / BATTLE_SPATIAL_CELL_SIZE).floor() as i32,
    )
}

fn battle_spatial_consider_cell(
    cells: &HashMap<(i32, i32), Vec<EntityId>>,
    cell: (i32, i32),
    actor: EntityId,
    actor_position: Vec3,
    positions: &HashMap<EntityId, Vec3>,
    best: &mut Option<(f64, EntityId)>,
) {
    let Some(candidates) = cells.get(&cell) else {
        return;
    };
    for &candidate in candidates {
        if candidate == actor {
            continue;
        }
        let candidate_position = positions[&candidate];
        let dx = candidate_position.x - actor_position.x;
        let dz = candidate_position.z - actor_position.z;
        let candidate_key = (dx * dx + dz * dz, candidate);
        if best.is_none_or(|current| {
            candidate_key.0.total_cmp(&current.0).is_lt()
                || (candidate_key.0 == current.0 && candidate_key.1 < current.1)
        }) {
            *best = Some(candidate_key);
        }
    }
}

fn exhaustive_battle_target(
    actor: EntityId,
    candidates: impl IntoIterator<Item = EntityId>,
    positions: &HashMap<EntityId, Vec3>,
) -> Option<EntityId> {
    let actor_position = positions.get(&actor)?;
    candidates
        .into_iter()
        .filter(|candidate| *candidate != actor)
        .map(|candidate| {
            let position = positions[&candidate];
            let dx = position.x - actor_position.x;
            let dz = position.z - actor_position.z;
            (dx * dx + dz * dz, candidate)
        })
        .min_by(|left, right| {
            left.0
                .total_cmp(&right.0)
                .then_with(|| left.1.cmp(&right.1))
        })
        .map(|(_, candidate)| candidate)
}

#[test]
fn battle_spatial_target_matches_exhaustive_nearest_with_deterministic_ties() {
    let mut state = 0xA11C_E5EED_u64;
    let mut positions = HashMap::new();
    for raw_id in 0..257 {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let x = (state as i32 % 20_001) as f64 / 10.0;
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let z = (state as i32 % 20_001) as f64 / 10.0;
        positions.insert(EntityId(raw_id), Vec3::new(x, 64.0, z));
    }
    let ids = positions.keys().copied().collect::<HashSet<_>>();
    let grid = BattleSpatialGrid::from_ids(ids.iter().copied(), &positions);
    for actor in 0..64 {
        let actor = EntityId(actor);
        assert_eq!(
            grid.choose_target(actor, &positions),
            exhaustive_battle_target(actor, ids.iter().copied(), &positions)
        );
    }

    let actor = EntityId(1_000);
    let lower_id = EntityId(1_001);
    let higher_id = EntityId(1_002);
    let tied_positions = HashMap::from([
        (actor, Vec3::new(0.0, 64.0, 0.0)),
        (lower_id, Vec3::new(-32.0, 64.0, 0.0)),
        (higher_id, Vec3::new(32.0, 64.0, 0.0)),
    ]);
    let tied_grid = BattleSpatialGrid::from_ids([higher_id, lower_id], &tied_positions);
    assert_eq!(
        tied_grid.choose_target(actor, &tied_positions),
        Some(lower_id)
    );
}

fn parse_linux_cpu_list(value: &str) -> Option<Vec<usize>> {
    let mut cpus = std::collections::BTreeSet::new();
    for part in value.trim().split(',') {
        if part.is_empty() {
            return None;
        }
        let mut bounds = part.split('-');
        let first = bounds.next()?.parse::<usize>().ok()?;
        let last = match bounds.next() {
            Some(last) => last.parse::<usize>().ok()?,
            None => first,
        };
        if first > last || bounds.next().is_some() {
            return None;
        }
        cpus.extend(first..=last);
    }
    (!cpus.is_empty()).then(|| cpus.into_iter().collect())
}

#[cfg(target_os = "linux")]
fn assert_benchmark_has_distinct_physical_cores(lanes: usize) {
    let Some(allowed) = std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|status| {
            status.lines().find_map(|line| {
                line.strip_prefix("Cpus_allowed_list:")
                    .and_then(parse_linux_cpu_list)
            })
        })
    else {
        return;
    };
    let mut physical = std::collections::BTreeSet::new();
    for cpu in allowed {
        let topology = format!("/sys/devices/system/cpu/cpu{cpu}/topology");
        let Ok(package) = std::fs::read_to_string(format!("{topology}/physical_package_id")) else {
            return;
        };
        let Ok(core) = std::fs::read_to_string(format!("{topology}/core_id")) else {
            return;
        };
        physical.insert((package.trim().to_owned(), core.trim().to_owned()));
    }
    assert!(
        physical.len() >= lanes,
        "regional scaling benchmark needs {lanes} distinct physical cores; allowed affinity exposes only {} (use physical siblings such as `taskset -c 0,2`, not SMT siblings `0,1`)",
        physical.len()
    );
}

#[test]
fn linux_cpu_list_parser_expands_ranges_and_singletons() {
    assert_eq!(
        parse_linux_cpu_list("0-2,5,8-9"),
        Some(vec![0, 1, 2, 5, 8, 9])
    );
    assert_eq!(parse_linux_cpu_list("2"), Some(vec![2]));
    assert_eq!(parse_linux_cpu_list("2-1"), None);
    assert_eq!(parse_linux_cpu_list("0,,2"), None);
}

#[derive(Default)]
struct TestDecisionJournalState {
    commits: Vec<super::RegionalCommitDecision>,
    cleared: Vec<super::RegionPhase>,
    fail_record: bool,
    fail_record_unknown: bool,
}

struct TestDecisionJournal(Arc<Mutex<TestDecisionJournalState>>);

impl super::RegionalDecisionJournal for TestDecisionJournal {
    fn record_commit(
        &mut self,
        decision: &super::RegionalCommitDecision,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        let mut state = self.0.lock().expect("test journal state");
        if state.fail_record {
            return Err(super::RegionalDecisionJournalError::SAFE);
        }
        state.commits.push(decision.clone());
        if state.fail_record_unknown {
            return Err(super::RegionalDecisionJournalError::OUTCOME_UNKNOWN);
        }
        Ok(())
    }

    fn clear_commit(
        &mut self,
        phase: super::RegionPhase,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        self.0
            .lock()
            .expect("test journal state")
            .cleared
            .push(phase);
        Ok(())
    }
}

struct BlockingClearDecisionJournal {
    state: Arc<Mutex<TestDecisionJournalState>>,
    clear_started: mpsc::SyncSender<()>,
    clear_release: mpsc::Receiver<()>,
}

struct BlockingRecordDecisionJournal {
    record_started: mpsc::SyncSender<()>,
    record_release: mpsc::Receiver<()>,
}

impl super::RegionalDecisionJournal for BlockingRecordDecisionJournal {
    fn record_commit(
        &mut self,
        _decision: &super::RegionalCommitDecision,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        self.record_started
            .send(())
            .expect("publish journal record start");
        self.record_release.recv().expect("release journal record");
        Ok(())
    }

    fn clear_commit(
        &mut self,
        _phase: super::RegionPhase,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        Ok(())
    }
}

impl super::RegionalDecisionJournal for BlockingClearDecisionJournal {
    fn record_commit(
        &mut self,
        decision: &super::RegionalCommitDecision,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        self.state
            .lock()
            .expect("test journal state")
            .commits
            .push(decision.clone());
        Ok(())
    }

    fn clear_commit(
        &mut self,
        phase: super::RegionPhase,
    ) -> Result<(), super::RegionalDecisionJournalError> {
        self.clear_commits(&[phase])
    }

    fn clear_commits(
        &mut self,
        phases: &[super::RegionPhase],
    ) -> Result<(), super::RegionalDecisionJournalError> {
        self.clear_started.send(()).expect("publish clear start");
        self.clear_release.recv().expect("release journal clear");
        self.state
            .lock()
            .expect("test journal state")
            .cleared
            .extend_from_slice(phases);
        Ok(())
    }
}

fn cow(position: Vec3) -> SpawnEntity {
    SpawnEntity::new(4, "minecraft:cow", position)
}

fn movement(id: EntityId, position: Vec3) -> EntityKinematics {
    EntityKinematics {
        id,
        position,
        rotation: Rotation {
            yaw: 15.0,
            pitch: -5.0,
            head_yaw: 12.0,
        },
        velocity: Vec3::new(0.4, 0.1, -0.2),
        on_ground: false,
    }
}

fn fenced_kinematics(
    coordinator: &super::RegionalOwnerCoordinator,
    ids: &HashSet<EntityId>,
) -> super::VersionedEntityKinematics {
    coordinator
        .alive_kinematics_for_ids(ids)
        .expect("fenced kinematics read")
        .1
        .expect("standalone unclaimed entities are fence eligible")
}

#[test]
fn versioned_kinematics_dense_same_region_matches_snapshot_cas() {
    let mut fast = super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
        .expect("fast coordinator");
    let mut cas = super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
        .expect("CAS coordinator");
    let spawns = (0..32)
        .map(|index| cow(Vec3::new(0.5 + f64::from(index), 64.0, 0.5)))
        .collect::<Vec<_>>();
    let fast_ids = fast.spawn_batch(spawns.clone()).expect("fast entities");
    let cas_ids = cas.spawn_batch(spawns).expect("CAS entities");
    assert_eq!(fast_ids, cas_ids);
    let ids = fast_ids.iter().copied().collect::<HashSet<_>>();
    let fence = fenced_kinematics(&fast, &ids);
    let targets = fence
        .expected
        .iter()
        .map(|entry| {
            movement(
                entry.id,
                Vec3::new(entry.expected.position.x + 0.25, 64.0, 0.5),
            )
        })
        .collect::<Vec<_>>();
    let expected = cas
        .snapshots_for_ids(&ids)
        .expect("CAS expected snapshots")
        .into_iter()
        .zip(targets.iter().copied())
        .collect::<Vec<_>>();

    assert_eq!(
        fast.apply_kinematics_if_versioned_inner(fence, targets.clone(), false)
            .expect("fast apply"),
        Some(targets.clone())
    );
    assert!(
        cas.apply_kinematics_if_current(expected)
            .expect("CAS apply")
    );
    let fast_states = fast.alive_kinematics_for_ids(&ids).expect("fast states").0;
    let cas_states = cas.alive_kinematics_for_ids(&ids).expect("CAS states").0;
    assert_eq!(fast_states, cas_states);
}

#[test]
fn versioned_kinematics_rejects_stale_lane_after_intervening_mutation() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let ids = HashSet::from([id]);
    let fence = fenced_kinematics(&coordinator, &ids);
    coordinator
        .set_velocities([(id, Vec3::new(0.1, 0.0, 0.0))])
        .expect("intervening velocity mutation");
    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("stale compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().position,
        Vec3::new(0.5, 64.0, 0.5)
    );
}

#[test]
fn versioned_kinematics_allows_unrelated_lane_mutation() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let ids = coordinator
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
        ])
        .expect("same-lane cows");
    let target = ids[0];
    let unrelated = ids[1];
    let fence = fenced_kinematics(&coordinator, &HashSet::from([target]));
    let unrelated_velocity = Vec3::new(0.1, 0.0, 0.0);
    coordinator
        .set_velocities([(unrelated, unrelated_velocity)])
        .expect("unrelated velocity mutation");

    let target_next = movement(target, Vec3::new(1.5, 64.0, 0.5));
    assert!(
        coordinator
            .apply_kinematics_if_versioned_inner(fence, vec![target_next], false)
            .expect("unrelated-lane compact compare")
            .is_some(),
        "disjoint lane traffic must not force a full snapshot fallback"
    );
    assert_eq!(
        coordinator.snapshot(target).unwrap().unwrap().position,
        target_next.position
    );
    assert_eq!(
        coordinator.snapshot(unrelated).unwrap().unwrap().velocity,
        unrelated_velocity
    );
}

#[test]
fn versioned_kinematics_rejects_changed_motion_even_with_current_version() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let mut fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    let changed_velocity = Vec3::new(0.25, 0.0, 0.0);
    coordinator
        .set_velocities([(id, changed_velocity)])
        .expect("changed-motion mutation");
    let lane = fence.routes[0].lane;
    fence
        .fence
        .lane_versions
        .insert(lane, coordinator.lanes[&lane].reader().state_version());

    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("compact compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().velocity,
        changed_velocity,
        "the intervening motion remains authoritative after compact rejection"
    );
}

#[test]
fn versioned_kinematics_rejects_same_region_attached_passenger_without_commit() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let passenger = coordinator
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("passenger");
    let boat = coordinator
        .spawn(SpawnEntity::vehicle(
            VehicleKind::Boat,
            1,
            "minecraft:oak_boat",
            Vec3::new(0.5, 64.0, 0.5),
        ))
        .expect("boat");
    let mut fence = fenced_kinematics(&coordinator, &HashSet::from([passenger]));

    let detached = coordinator.snapshot(boat).unwrap().unwrap();
    let mut attached = detached.clone();
    attached.vehicle.as_mut().expect("boat state").passenger = Some(passenger);
    assert!(
        coordinator
            .replace_snapshot_if_current(detached, attached)
            .expect("attach passenger")
    );

    let lane = fence.routes[0].lane;
    fence
        .fence
        .lane_versions
        .insert(lane, coordinator.lanes[&lane].reader().state_version());
    let target = movement(passenger, Vec3::new(2.5, 64.0, 0.5));

    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(fence, vec![target], false)
            .expect("attached-passenger compact compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(boat).unwrap().unwrap().position,
        Vec3::new(0.5, 64.0, 0.5)
    );
    assert_eq!(
        coordinator.snapshot(passenger).unwrap().unwrap().position,
        Vec3::new(1.5, 64.0, 0.5)
    );
}

#[test]
fn versioned_kinematics_rejects_intervening_pickup_claim_without_commit() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("entity");
    let mut fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    let current = coordinator.snapshot(id).unwrap().unwrap();
    let mut claimed = current.clone();
    claimed.retained.item_pickup_claim = Some(77);
    assert!(
        coordinator
            .replace_snapshot_if_current(current, claimed)
            .expect("install pickup claim")
    );

    let lane = fence.routes[0].lane;
    fence
        .fence
        .lane_versions
        .insert(lane, coordinator.lanes[&lane].reader().state_version());
    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("claimed compact compare"),
        None
    );
    let snapshot = coordinator.snapshot(id).unwrap().unwrap();
    assert_eq!(snapshot.position, Vec3::new(0.5, 64.0, 0.5));
    assert_eq!(snapshot.retained.item_pickup_claim, Some(77));
}

#[test]
fn versioned_kinematics_rejects_reused_id_uuid_mismatch() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let mut fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    let original_uuid = fence.expected[0].uuid;
    fence.expected[0].uuid = if original_uuid == Uuid::nil() {
        Uuid::max()
    } else {
        Uuid::nil()
    };

    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("identity compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().position,
        Vec3::new(0.5, 64.0, 0.5)
    );
}

#[test]
fn versioned_kinematics_stale_lane_does_not_block_fresh_lane() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("coordinator");
    let ids = coordinator
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("two-lane cows");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let fence = fenced_kinematics(&coordinator, &selected);
    coordinator
        .set_velocities([(ids[1], Vec3::new(0.25, 0.0, 0.0))])
        .expect("stale-lane mutation");
    let fresh_target = movement(ids[0], Vec3::new(1.5, 64.0, 0.5));
    let stale_target = movement(ids[1], Vec3::new(129.5, 64.0, 0.5));

    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(fence, vec![fresh_target, stale_target], false,)
            .expect("independent lane compact compare"),
        Some(vec![fresh_target])
    );
    assert_eq!(
        coordinator.snapshot(ids[0]).unwrap().unwrap().position,
        Vec3::new(1.5, 64.0, 0.5)
    );
    assert_eq!(
        coordinator.snapshot(ids[1]).unwrap().unwrap().position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn concurrent_duplicate_compact_submission_exactly_one_succeeds() {
    let runtime =
        super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1).expect("runtime");
    let handle = runtime.handle();
    let id = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let ids = HashSet::from([id]);
    let resolved = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(ids.clone()))
        .expect("goal prepare")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let (_, _, versioned) = handle
        .apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(
            resolved,
            Arc::new(ids.clone()),
        )
        .expect("goal apply")
        .expect("fresh goal apply");
    let fence = versioned.expect("compact fence");
    let target = movement(id, Vec3::new(1.5, 64.0, 0.5));
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let submissions = (0..2)
        .map(|_| {
            let handle = handle.clone();
            let fence = fence.clone();
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                handle
                    .try_apply_kinematics_if_versioned_deferred_journal(fence, vec![target])
                    .expect("compact submission")
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();
    let succeeded = submissions
        .into_iter()
        .filter_map(|submission| submission.join().expect("submission thread"))
        .count();

    assert_eq!(succeeded, 1);
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn route_invalidation_cannot_commit_old_compact_batch() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    assert!(
        coordinator
            .set_position(id, Vec3::new(128.5, 64.0, 0.5))
            .expect("route mutation")
    );

    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("old route compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn fenced_kinematics_lane_reply_keeps_pre_mutation_version() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let key = coordinator.locations[&id];
    let lease = coordinator.ownership.lease(key).expect("cow lease");
    let reader = coordinator.lanes[&lease.lane].reader();
    let completion = reader
        .request_alive_kinematics_for_ids(vec![(lease, id)])
        .expect("lane kinematics request");
    let read = completion.recv().expect("lane reply").expect("lane read");
    let previous = read.candidates[0]
        .expected()
        .expect("living candidate has simulation result");

    coordinator
        .set_velocities([(id, Vec3::new(0.25, 0.0, 0.0))])
        .expect("post-read mutation");

    assert_ne!(
        read.state_version,
        reader.state_version(),
        "the reply keeps the version captured with its old candidate batch"
    );
    let mutation = super::RegionOwnerMutation::SetKinematicsBatchIfVersion {
        expected_state_version: read.state_version,
        previous: vec![previous],
        states: vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
    };
    assert_eq!(super::mutation_entity_ids(&mutation), vec![id]);
}

#[test]
fn versioned_kinematics_region_crossing_falls_back_without_commit() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(128.5, 64.0, 0.5))],
                false,
            )
            .expect("crossing compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().position,
        Vec3::new(0.5, 64.0, 0.5)
    );
}

#[test]
fn versioned_kinematics_incomplete_route_falls_back_without_commit() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let id = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let mut fence = fenced_kinematics(&coordinator, &HashSet::from([id]));
    fence.routes.clear();
    assert_eq!(
        coordinator
            .apply_kinematics_if_versioned_inner(
                fence,
                vec![movement(id, Vec3::new(1.5, 64.0, 0.5))],
                false,
            )
            .expect("incomplete route compare"),
        None
    );
    assert_eq!(
        coordinator.snapshot(id).unwrap().unwrap().position,
        Vec3::new(0.5, 64.0, 0.5)
    );
}

#[test]
fn post_goal_fence_excludes_vehicle_groups_and_pickup_claims() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("coordinator");
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        1,
        "minecraft:oak_boat",
        Vec3::new(0.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let vehicle_ids = coordinator
        .spawn_batch([boat, cow(Vec3::new(1.5, 64.0, 0.5))])
        .expect("vehicle and passenger batch");
    let boat = vehicle_ids[0];
    let passenger = vehicle_ids[1];
    let mut claimed = cow(Vec3::new(2.5, 64.0, 0.5));
    claimed.retained.item_pickup_claim = Some(77);
    let claimed = coordinator.spawn(claimed).expect("claimed entity");

    assert!(
        coordinator
            .alive_kinematics_for_ids(&HashSet::from([boat, passenger]))
            .expect("vehicle read")
            .1
            .is_none()
    );
    assert!(
        coordinator
            .alive_kinematics_for_ids(&HashSet::from([claimed]))
            .expect("claimed read")
            .1
            .is_none()
    );
}

struct WalkablePathing;

impl PathingProbe for WalkablePathing {
    fn can_stand_at(&self, _position: Vec3) -> PathingProbeResult {
        PathingProbeResult::Walkable
    }
}

struct ConcurrentPathingProbe {
    entered: mpsc::Sender<EntityId>,
    release: Mutex<mpsc::Receiver<()>>,
    blocked_once: Mutex<HashSet<EntityId>>,
}

impl PathingProbe for ConcurrentPathingProbe {
    fn can_stand_at(&self, position: Vec3) -> PathingProbeResult {
        self.can_entity_stand_at(EntityId(0), position)
    }

    fn can_entity_stand_at(&self, entity_id: EntityId, _position: Vec3) -> PathingProbeResult {
        let first_call = self
            .blocked_once
            .lock()
            .expect("pathing call set")
            .insert(entity_id);
        if first_call {
            self.entered.send(entity_id).expect("publish entered job");
            self.release
                .lock()
                .expect("pathing release receiver")
                .recv()
                .expect("receive exact job release");
        }
        PathingProbeResult::Walkable
    }
}

#[test]
fn regional_entity_stores_allocate_global_ids_and_keep_state_separate() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");

    let west_id = regions
        .spawn(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
        .expect("west cow");
    let east_id = regions
        .spawn(phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");

    assert_ne!(west_id, east_id);
    assert_eq!(regions.region_for(west_id), Some(west.key));
    assert_eq!(regions.region_for(east_id), Some(east.key));
    assert_eq!(regions.region_len(west.key), 1);
    assert_eq!(regions.region_len(east.key), 1);
    assert_eq!(
        regions.snapshot(west_id).expect("west snapshot").position,
        Vec3::new(-0.5, 64.0, 0.5)
    );
    assert_eq!(
        regions.snapshot(east_id).expect("east snapshot").position,
        Vec3::new(128.5, 64.0, 0.5)
    );
    regions
        .acknowledge_lane(phase, 0)
        .expect("lane 0 completes spawn phase");
    regions.finish_phase(phase).expect("finish spawn phase");

    let reassigned_east = regions
        .reassign_region(east, 1)
        .expect("move east region to lane 1");
    let next_phase = regions.begin_phase().expect("next phase");
    assert_eq!(
        regions.spawn(next_phase, east, cow(Vec3::new(129.5, 64.0, 0.5)),),
        Err(RegionEntityStoreError::StaleLease)
    );
    assert!(
        regions
            .spawn(
                next_phase,
                reassigned_east,
                cow(Vec3::new(129.5, 64.0, 0.5)),
            )
            .is_ok()
    );
    regions
        .acknowledge_lane(next_phase, 0)
        .expect("lane 0 completes next phase");
    regions
        .acknowledge_lane(next_phase, 1)
        .expect("lane 1 completes next phase");
    regions.finish_phase(next_phase).expect("finish next phase");
}

#[test]
fn regional_entity_stores_allow_remote_follow_but_reject_cross_region_vehicle_links() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let spawn_phase = regions.begin_phase().expect("spawn phase");
    let east_id = regions
        .spawn(spawn_phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let mut follower = cow(Vec3::new(-0.5, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target: east_id,
        speed: 0.25,
    };
    assert!(regions.spawn(spawn_phase, west, follower).is_ok());
    let mut boat = cow(Vec3::new(-1.5, 64.0, 0.5));
    boat.vehicle = Some(VehicleState {
        kind: VehicleKind::Boat,
        passenger: Some(east_id),
    });
    assert_eq!(
        regions.spawn(spawn_phase, west, boat),
        Err(RegionEntityStoreError::CrossRegionReference)
    );
    assert_eq!(
        regions.spawn(spawn_phase, west, cow(Vec3::new(0.5, 64.0, 0.5)),),
        Err(RegionEntityStoreError::WrongSpawnRegion)
    );
    assert_eq!(regions.region_len(west.key), 1);
    assert_eq!(regions.region_len(east.key), 1);
    regions
        .acknowledge_lane(spawn_phase, 0)
        .expect("lane 0 completes spawn phase");
    regions
        .finish_phase(spawn_phase)
        .expect("finish spawn phase");

    assert_eq!(
        regions.remove(spawn_phase, east, east_id),
        Err(RegionEntityStoreError::StalePhase)
    );
    assert!(regions.snapshot(east_id).is_some());
}

#[test]
fn regional_entity_store_rejects_duplicate_uuid_without_consuming_id() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let uuid = Uuid::from_u128(42);
    let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
    east_cow.uuid = Some(uuid);
    assert_eq!(
        regions.spawn(phase, east, east_cow).expect("east cow"),
        EntityId(1)
    );
    let mut duplicate = cow(Vec3::new(-0.5, 64.0, 0.5));
    duplicate.uuid = Some(uuid);
    assert_eq!(
        regions.spawn(phase, west, duplicate),
        Err(RegionEntityStoreError::DuplicateUuid)
    );
    assert_eq!(
        regions
            .spawn(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
            .expect("next valid cow"),
        EntityId(2)
    );
    regions
        .acknowledge_lane(phase, 0)
        .expect("lane 0 completes spawn phase");
    regions.finish_phase(phase).expect("finish spawn phase");
}

#[test]
fn committed_transfer_moves_authority_once_and_replays_idempotently() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let spawn_phase = regions.begin_phase().expect("spawn phase");
    let id = regions
        .spawn(spawn_phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    let uuid = regions.snapshot(id).expect("source cow").uuid;
    regions
        .acknowledge_lane(spawn_phase, 0)
        .expect("west spawn complete");
    regions
        .acknowledge_lane(spawn_phase, 1)
        .expect("east spawn complete");
    regions.finish_phase(spawn_phase).expect("finish spawn");

    let phase = regions.begin_phase().expect("migration phase");
    let transfer = regions
        .prepare_transfer(
            phase,
            west,
            east,
            41,
            movement(id, Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepare transfer");
    assert_eq!(
        regions
            .prepare_transfer(
                phase,
                west,
                east,
                41,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("replay prepare"),
        transfer
    );
    assert_eq!(regions.region_for(id), Some(west.key));
    assert_eq!(regions.region_len(west.key), 1);
    assert_eq!(regions.region_len(east.key), 0);
    assert_eq!(
        regions.decide_transfer(phase, transfer, TransferDecision::Commit),
        Ok(())
    );
    assert_eq!(
        regions
            .prepare_transfer(
                phase,
                west,
                east,
                41,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare after decision"),
        transfer
    );
    assert_eq!(
        regions.finish_phase(phase),
        Err(RegionEntityStoreError::Ownership(
            RegionOwnershipError::PhaseIncomplete
        ))
    );
    assert_eq!(
        regions.decide_transfer(phase, transfer, TransferDecision::Commit),
        Ok(())
    );
    assert_eq!(
        regions.decide_transfer(phase, transfer, TransferDecision::Reject),
        Err(RegionEntityStoreError::DecisionConflict)
    );
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Committed)
    );
    assert_eq!(
        regions
            .prepare_transfer(
                phase,
                west,
                east,
                41,
                movement(id, Vec3::new(128.25, 64.0, 0.5)),
            )
            .expect("prepare after apply"),
        transfer
    );
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Committed)
    );
    let migrated = regions.snapshot(id).expect("destination cow");
    assert_eq!(migrated.id, id);
    assert_eq!(migrated.uuid, uuid);
    assert_eq!(migrated.position, Vec3::new(128.25, 64.0, 0.5));
    assert_eq!(
        migrated.rotation,
        Rotation {
            yaw: 15.0,
            pitch: -5.0,
            head_yaw: 12.0,
        }
    );
    assert_eq!(migrated.velocity, Vec3::new(0.4, 0.1, -0.2));
    assert!(!migrated.on_ground);
    assert_eq!(regions.region_for(id), Some(east.key));
    assert_eq!(regions.region_len(west.key), 0);
    assert_eq!(regions.region_len(east.key), 1);
    regions
        .acknowledge_lane(phase, 0)
        .expect("west migration complete");
    regions
        .acknowledge_lane(phase, 1)
        .expect("east migration complete");
    regions.finish_phase(phase).expect("finish migration");
    assert!(regions.transfers.is_empty());
    assert!(regions.in_flight_transfers.is_empty());
}

#[test]
fn rejected_transfer_keeps_source_authority_and_replays_idempotently() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let id = regions
        .spawn(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    let transfer = regions
        .prepare_transfer(
            phase,
            west,
            east,
            7,
            movement(id, Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepare transfer");
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Err(RegionEntityStoreError::TransferUndecided)
    );
    assert_eq!(
        regions.remove(phase, west, id),
        Err(RegionEntityStoreError::TransferConflict)
    );
    regions
        .decide_transfer(phase, transfer, TransferDecision::Reject)
        .expect("reject transfer");
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Rejected)
    );
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Rejected)
    );
    assert_eq!(regions.region_for(id), Some(west.key));
    assert_eq!(regions.region_len(west.key), 1);
    assert_eq!(regions.region_len(east.key), 0);
    assert_eq!(
        regions.snapshot(id).expect("source cow").position,
        Vec3::new(127.5, 64.0, 0.5)
    );
    regions
        .acknowledge_lane(phase, 0)
        .expect("west rejection complete");
    regions
        .acknowledge_lane(phase, 1)
        .expect("east rejection complete");
    regions.finish_phase(phase).expect("finish rejection");
}

#[test]
fn phase_boundary_rejects_undecided_transfer_and_closes_exactly_once() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let id = regions
        .spawn(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    regions
        .prepare_transfer(
            phase,
            west,
            east,
            9,
            movement(id, Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepare transfer");
    regions.acknowledge_lane(phase, 0).expect("west complete");
    regions.acknowledge_lane(phase, 1).expect("east complete");
    regions.finish_phase(phase).expect("implicit reject");
    assert_eq!(regions.region_for(id), Some(west.key));
    assert_eq!(
        regions.finish_phase(phase),
        Err(RegionEntityStoreError::Ownership(
            RegionOwnershipError::PhaseInactive
        ))
    );
}

#[test]
fn acknowledged_lane_cannot_prepare_late_transfer() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let id = regions
        .spawn(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    regions.acknowledge_lane(phase, 0).expect("west complete");
    assert_eq!(
        regions.prepare_transfer(
            phase,
            west,
            east,
            10,
            movement(id, Vec3::new(128.25, 64.0, 0.5)),
        ),
        Err(RegionEntityStoreError::Ownership(
            RegionOwnershipError::LaneCompleted
        ))
    );
    assert_eq!(regions.region_for(id), Some(west.key));
}

#[test]
fn transfer_allows_remote_follower_to_remain_in_source_region() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let target = regions
        .spawn(phase, west, cow(Vec3::new(127.0, 64.0, 0.5)))
        .expect("target cow");
    let mut follower = cow(Vec3::new(126.0, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target,
        speed: 0.25,
    };
    let follower = regions
        .spawn(phase, west, follower)
        .expect("source follower");
    let transfer = regions
        .prepare_transfer(
            phase,
            west,
            east,
            11,
            movement(target, Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepare target crossing");
    regions
        .decide_transfer(phase, transfer, TransferDecision::Commit)
        .expect("commit target crossing");
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Committed)
    );
    assert_eq!(regions.region_for(target), Some(east.key));
    assert_eq!(regions.region_for(follower), Some(west.key));
}

#[test]
fn prepared_vehicle_group_blocks_new_vehicle_links_but_allows_remote_followers() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = regions
        .spawn_batch(
            phase,
            [
                boat,
                cow(Vec3::new(127.75, 64.0, 0.5)),
                cow(Vec3::new(126.5, 64.0, 0.5)),
            ],
        )
        .expect("vehicle group and follower");
    let transfer = regions
        .prepare_transfer(
            phase,
            west,
            east,
            12,
            movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepare vehicle group");
    let follow_passenger = GoalState::FollowTarget {
        target: ids[1],
        speed: 0.25,
    };

    assert_eq!(
        regions.set_goal(phase, ids[2], follow_passenger.clone()),
        Ok(true)
    );
    let mut new_follower = cow(Vec3::new(126.0, 64.0, 0.5));
    new_follower.goal = follow_passenger.clone();
    assert!(regions.spawn(phase, west, new_follower).is_ok());
    let mut second_boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(126.0, 64.0, 1.5),
    );
    second_boat
        .vehicle
        .as_mut()
        .expect("second boat state")
        .passenger = Some(ids[1]);
    assert_eq!(
        regions.spawn(phase, west, second_boat),
        Err(RegionEntityStoreError::TransferConflict)
    );
    assert_eq!(
        regions.set_velocity(phase, ids[1], Vec3::new(0.1, 0.0, 0.0)),
        Err(RegionEntityStoreError::TransferConflict)
    );

    regions
        .decide_transfer(phase, transfer, TransferDecision::Reject)
        .expect("reject group");
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Rejected)
    );
    assert_eq!(regions.set_goal(phase, ids[2], follow_passenger), Ok(true));
    assert_eq!(
        regions.set_velocity(phase, ids[1], Vec3::new(0.1, 0.0, 0.0)),
        Ok(true)
    );
    assert_eq!(
        regions
            .snapshot(ids[0])
            .and_then(|snapshot| snapshot.vehicle)
            .and_then(|vehicle| vehicle.passenger),
        Some(ids[1])
    );
}

#[test]
fn regional_kinematics_apply_local_motion_and_prepare_boundary_transfer() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let local = regions
        .spawn(phase, west, cow(Vec3::new(10.0, 64.0, 0.5)))
        .expect("local cow");
    let crossing = regions
        .spawn(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");

    assert_eq!(
        regions.apply_kinematics(phase, 17, movement(local, Vec3::new(11.0, 64.0, 0.5))),
        Ok(super::RegionalKinematicsApply::AppliedLocal)
    );
    let transfer = match regions
        .apply_kinematics(phase, 17, movement(crossing, Vec3::new(128.25, 64.0, 0.5)))
        .expect("prepare crossing")
    {
        super::RegionalKinematicsApply::PreparedTransfer(transfer) => transfer,
        other => panic!("expected prepared transfer, got {other:?}"),
    };
    assert_eq!(
        regions.snapshot(local).expect("local motion").position,
        Vec3::new(11.0, 64.0, 0.5)
    );
    assert_eq!(regions.region_for(crossing), Some(west.key));
    assert_eq!(
        regions.snapshot(crossing).expect("source motion").position,
        Vec3::new(127.5, 64.0, 0.5)
    );
    assert_eq!(
        regions.apply_kinematics(phase, 17, movement(crossing, Vec3::new(127.75, 64.0, 0.5)),),
        Err(RegionEntityStoreError::TransferConflict)
    );
    regions
        .decide_transfer(phase, transfer, TransferDecision::Commit)
        .expect("commit crossing");
    assert_eq!(
        regions.apply_transfer(phase, transfer),
        Ok(TransferApply::Committed)
    );
    assert_eq!(regions.region_for(crossing), Some(east.key));
}

#[test]
fn regional_global_reads_and_visitors_keep_entity_id_order() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
    east_cow.animal = Some(AnimalBreedingState::baby());
    let east_id = regions.spawn(phase, east, east_cow).expect("east cow");
    let mut west_sheep = SpawnEntity::new(5, "minecraft:sheep", Vec3::new(-0.5, 64.0, 0.5));
    west_sheep.animal = Some(AnimalBreedingState::adult_sheep(SheepColor::White));
    let west_id = regions.spawn(phase, west, west_sheep).expect("west sheep");
    let east_uuid = regions.snapshot(east_id).expect("east snapshot").uuid;

    assert_eq!(regions.len(), 2);
    assert!(!regions.is_empty());
    assert!(regions.contains(east_id));
    assert!(regions.contains_uuid(east_uuid));
    assert_eq!(
        regions
            .snapshots()
            .map(|entity| entity.id)
            .collect::<Vec<_>>(),
        vec![east_id, west_id]
    );
    assert_eq!(
        regions.motion_state(west_id).expect("west motion").position,
        Vec3::new(-0.5, 64.0, 0.5)
    );

    let mut all = Vec::new();
    regions.visit_simulation_entities(|entity| all.push(entity.id));
    assert_eq!(all, vec![east_id, west_id]);
    let mut selected = Vec::new();
    regions.visit_simulation_entities_for_ids(&HashSet::from([west_id, east_id]), |entity| {
        selected.push(entity.id)
    });
    assert_eq!(selected, vec![east_id, west_id]);
    let mut sheep = Vec::new();
    regions.visit_sheep_entities_for_ids(&HashSet::from([east_id, west_id]), |entity| {
        sheep.push(entity.id)
    });
    assert_eq!(sheep, vec![west_id]);

    assert_eq!(
        regions.set_velocity(phase, west_id, Vec3::new(0.2, 0.0, 0.1)),
        Ok(true)
    );
    assert_eq!(
        regions
            .motion_state(west_id)
            .expect("updated west motion")
            .velocity,
        Vec3::new(0.2, 0.0, 0.1)
    );
    assert_eq!(
        regions.set_animal_state(phase, east_id, AnimalBreedingState::adult()),
        Ok(true)
    );
    assert_eq!(regions.set_goal(phase, east_id, GoalState::Idle), Ok(true));
    assert_eq!(
        regions.set_goal(
            phase,
            east_id,
            GoalState::FollowTarget {
                target: west_id,
                speed: 0.25,
            },
        ),
        Ok(true)
    );
    assert!(
        regions
            .damage(phase, east_id, damage_request(1.0))
            .expect("damage")
            .is_some()
    );
}

#[test]
fn regional_batch_spawn_is_atomic_across_regions() {
    let mut regions = RegionalEntityStore::new();
    regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let ids = regions
        .spawn_batch(
            phase,
            [
                cow(Vec3::new(128.5, 64.0, 0.5)),
                cow(Vec3::new(-0.5, 64.0, 0.5)),
            ],
        )
        .expect("cross-region batch");
    assert_eq!(ids, vec![EntityId(1), EntityId(2)]);
    assert_eq!(regions.region_for(ids[0]), Some(RegionKey::new(1, 0)));
    assert_eq!(regions.region_for(ids[1]), Some(RegionKey::new(-1, 0)));

    let duplicate_uuid = Uuid::from_u128(9001);
    let mut first = cow(Vec3::new(129.5, 64.0, 0.5));
    first.uuid = Some(duplicate_uuid);
    let mut second = cow(Vec3::new(-1.5, 64.0, 0.5));
    second.uuid = Some(duplicate_uuid);
    assert_eq!(
        regions.spawn_batch(phase, [first, second]),
        Err(RegionEntityStoreError::DuplicateUuid)
    );
    assert_eq!(regions.len(), 2);
    assert_eq!(
        regions
            .spawn_batch(phase, [cow(Vec3::new(130.5, 64.0, 0.5))])
            .expect("id after rejected batch"),
        vec![EntityId(3)]
    );
}

#[test]
fn regional_restore_preserves_ids_and_rejects_cross_region_graph_atomically() {
    let mut source = RegionalEntityStore::new();
    let west = source
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("source west");
    let east = source
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("source east");
    let source_phase = source.begin_phase().expect("source phase");
    let east_id = source
        .spawn(source_phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let west_id = source
        .spawn(source_phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
        .expect("west cow");
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(-1.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(west_id);
    source.spawn(source_phase, west, boat).expect("west boat");
    let snapshots = source.snapshots().collect::<Vec<_>>();

    let mut restored = RegionalEntityStore::new();
    restored
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("restore west");
    restored
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("restore east");
    let phase = restored.begin_phase().expect("restore phase");
    assert_eq!(restored.insert_snapshots(phase, snapshots.clone()), Ok(3));
    assert_eq!(restored.snapshots().collect::<Vec<_>>(), snapshots);

    let mut invalid = snapshots.clone();
    invalid[0].vehicle = Some(VehicleState {
        kind: VehicleKind::Boat,
        passenger: Some(west_id),
    });
    let mut rejected = RegionalEntityStore::new();
    rejected
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("rejected west");
    rejected
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("rejected east");
    let rejected_phase = rejected.begin_phase().expect("rejected phase");
    assert_eq!(
        rejected.insert_snapshots(rejected_phase, invalid),
        Err(RegionEntityStoreError::CrossRegionReference)
    );
    assert!(rejected.is_empty());

    let mut duplicate_vehicle = snapshots;
    let mut second_boat = duplicate_vehicle[2].clone();
    second_boat.id = EntityId(4);
    second_boat.uuid = Uuid::from_u128(9002);
    second_boat.position = Vec3::new(-2.5, 64.0, 0.5);
    duplicate_vehicle.push(second_boat);
    let mut graph_rejected = RegionalEntityStore::new();
    graph_rejected
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("graph west");
    graph_rejected
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("graph east");
    let graph_phase = graph_rejected.begin_phase().expect("graph phase");
    assert_eq!(
        graph_rejected.insert_snapshots(graph_phase, duplicate_vehicle),
        Err(RegionEntityStoreError::TargetConflict)
    );
    assert!(graph_rejected.is_empty());
    assert_eq!(east_id, EntityId(1));
}

#[test]
fn batch_cannot_add_vehicle_reference_to_entity_in_transfer() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");
    let target = regions
        .spawn(phase, west, cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("target cow");
    regions
        .prepare_transfer(
            phase,
            west,
            east,
            88,
            movement(target, Vec3::new(128.25, 64.0, 0.5)),
        )
        .expect("prepared target");
    let mut follower = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(126.5, 64.0, 0.5),
    );
    follower.vehicle.as_mut().expect("boat state").passenger = Some(target);
    assert_eq!(
        regions.spawn_batch(phase, [follower]),
        Err(RegionEntityStoreError::TransferConflict)
    );
    assert_eq!(regions.len(), 1);
}

#[test]
fn regional_goal_batch_prepares_and_applies_across_regions() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("phase");

    let mut west_cow = cow(Vec3::new(-0.5, 64.0, 0.5));
    west_cow.goal = GoalState::FollowPosition {
        target: Vec3::new(-2.5, 64.0, 0.5),
        speed: 0.25,
    };
    let west_id = regions.spawn(phase, west, west_cow).expect("west cow");
    let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
    east_cow.goal = GoalState::FollowPosition {
        target: Vec3::new(130.5, 64.0, 0.5),
        speed: 0.25,
    };
    let east_id = regions.spawn(phase, east, east_cow).expect("east cow");

    let prepared = regions
        .prepare_goal_tick_with_pathing_for_ids(phase, 17, &HashSet::from([west_id, east_id]))
        .expect("prepare regional goals");
    let resolved = prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let stats = regions
        .apply_prepared_goal_tick(phase, resolved)
        .expect("apply regional goals");

    assert_eq!(stats.alive_entities, 2);
    assert_eq!(stats.decisions_applied, 2);
    assert_eq!(stats.pathing_moves, 2);
    assert_ne!(
        regions.motion_state(west_id).expect("west motion").velocity,
        Vec3::ZERO
    );
    assert_ne!(
        regions.motion_state(east_id).expect("east motion").velocity,
        Vec3::ZERO
    );
}

#[test]
fn regional_goal_parallel_batch_count_ignores_idle_regions() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("goal phase");
    let west_id = regions
        .spawn(phase, west, cow(Vec3::new(-0.5, 64.0, 0.5)))
        .expect("west cow");
    let east_id = regions
        .spawn(phase, east, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");

    let prepared = regions
        .prepare_goal_tick_with_pathing_for_ids(phase, 31, &HashSet::from([west_id, east_id]))
        .expect("prepare regional goals");

    assert_eq!(prepared.parallel_batch_count(), 0);
}

#[test]
fn regional_goal_resolve_runs_independent_regions_concurrently() {
    let mut regions = RegionalEntityStore::new();
    let west = regions
        .assign_region(RegionKey::new(-1, 0), 0)
        .expect("west region");
    let east = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("east region");
    let phase = regions.begin_phase().expect("goal phase");
    let mut west_cow = cow(Vec3::new(-0.5, 64.0, 0.5));
    west_cow.goal = GoalState::Wander {
        speed: 0.25,
        period_ticks: 1,
    };
    let mut east_cow = cow(Vec3::new(128.5, 64.0, 0.5));
    east_cow.goal = GoalState::Wander {
        speed: 0.25,
        period_ticks: 1,
    };
    let west_id = regions.spawn(phase, west, west_cow).expect("west cow");
    let east_id = regions.spawn(phase, east, east_cow).expect("east cow");
    let prepared = regions
        .prepare_goal_tick_with_pathing_for_ids(phase, 31, &HashSet::from([west_id, east_id]))
        .expect("prepare regional goals");
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let probe = Arc::new(ConcurrentPathingProbe {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        blocked_once: Mutex::new(HashSet::new()),
    });
    let worker_probe = Arc::clone(&probe);
    let worker = std::thread::spawn(move || {
        prepared.resolve_parallel(&*worker_probe, PathingBudget::DEFAULT, 2)
    });

    let first = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first region entered");
    let second = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second region entered before either was released");
    assert_ne!(first, second);
    release_tx.send(()).expect("release first region");
    release_tx.send(()).expect("release second region");
    let resolved = worker.join().expect("regional resolve worker");
    let stats = regions
        .apply_prepared_goal_tick(phase, resolved)
        .expect("apply regional goals");
    assert_eq!(stats.decisions_applied, 2);
}

#[test]
fn production_goal_apply_runs_independent_regions_concurrently() {
    let mut authority = RegionalEntityAuthority::default();
    let mut west = cow(Vec3::new(-0.5, 64.0, 0.5));
    west.goal = GoalState::Wander {
        speed: 0.25,
        period_ticks: 1,
    };
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.goal = GoalState::Wander {
        speed: 0.25,
        period_ticks: 1,
    };
    let ids = authority.spawn_batch([west, east]);
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(32, &ids.into_iter().collect())
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);

    let worker = std::thread::spawn(move || {
        authority.apply_prepared_goal_tick_parallel_with_probe(resolved, 2, &|key| {
            entered_tx.send(key).expect("goal apply probe receiver");
            release_rx
                .lock()
                .expect("regional test lock poisoned")
                .recv()
                .expect("goal apply probe release");
        })
    });

    let first = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first goal region entered");
    let second = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second goal region entered before either was released");
    assert_ne!(first, second);
    release_tx.send(()).expect("release first goal region");
    release_tx.send(()).expect("release second goal region");
    let stats = worker.join().expect("regional goal apply worker");
    assert_eq!(stats.decisions_applied, 2);
}

#[test]
fn regional_local_kinematics_apply_runs_independent_regions_concurrently() {
    let mut authority = RegionalEntityAuthority::default();
    let ids = authority.spawn_batch([
        cow(Vec3::new(-0.5, 64.0, 0.5)),
        cow(Vec3::new(128.5, 64.0, 0.5)),
    ]);
    let states = [
        movement(ids[0], Vec3::new(-0.25, 64.0, 0.5)),
        movement(ids[1], Vec3::new(128.75, 64.0, 0.5)),
    ];
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);

    let worker = std::thread::spawn(move || {
        authority.apply_kinematics_parallel_with_probe(states, 2, &|key| {
            entered_tx.send(key).expect("kinematics probe receiver");
            release_rx
                .lock()
                .expect("regional test lock poisoned")
                .recv()
                .expect("kinematics probe release");
        })
    });

    let first = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("first region entered");
    let second = entered_rx
        .recv_timeout(Duration::from_secs(2))
        .expect("second region entered before either was released");
    assert_ne!(first, second);
    release_tx.send(()).expect("release first region");
    release_tx.send(()).expect("release second region");
    assert_eq!(worker.join().expect("regional apply worker"), 2);
}

#[test]
fn production_kinematics_parallelism_skips_small_batches() {
    let mut authority = RegionalEntityAuthority::default();
    let ids = authority.spawn_batch([
        cow(Vec3::new(-0.5, 64.0, 0.5)),
        cow(Vec3::new(128.5, 64.0, 0.5)),
    ]);
    let states = [
        movement(ids[0], Vec3::new(-0.25, 64.0, 0.5)),
        movement(ids[1], Vec3::new(128.75, 64.0, 0.5)),
    ];

    assert_eq!(authority.parallel_kinematics_batch_count(&states), 0);
}

#[test]
fn production_kinematics_parallelism_admits_dense_regions() {
    let entities = (0..257).map(|index| {
        if index < 128 {
            cow(Vec3::new(-0.5, 64.0, 0.5))
        } else {
            cow(Vec3::new(128.5, 64.0, 0.5))
        }
    });
    let mut authority = RegionalEntityAuthority::default();
    let ids = authority.spawn_batch(entities);
    let states = ids
        .into_iter()
        .enumerate()
        .map(|(index, id)| {
            movement(
                id,
                if index < 128 {
                    Vec3::new(-0.25, 64.0, 0.5)
                } else {
                    Vec3::new(128.75, 64.0, 0.5)
                },
            )
        })
        .collect::<Vec<_>>();

    assert_eq!(authority.parallel_kinematics_batch_count(&states), 2);
}

#[test]
fn regional_goal_batch_rejects_a_result_from_an_old_phase() {
    let mut regions = RegionalEntityStore::new();
    let lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("region");
    let first_phase = regions.begin_phase().expect("first phase");
    let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
    entity.goal = GoalState::FollowPosition {
        target: Vec3::new(2.5, 64.0, 0.5),
        speed: 0.25,
    };
    let id = regions.spawn(first_phase, lease, entity).expect("cow");
    let prepared = regions
        .prepare_goal_tick_with_pathing_for_ids(first_phase, 18, &HashSet::from([id]))
        .expect("prepare goals");
    let resolved = prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT);
    regions
        .acknowledge_lane(first_phase, 0)
        .expect("ack first phase");
    regions
        .finish_phase(first_phase)
        .expect("finish first phase");
    let second_phase = regions.begin_phase().expect("second phase");

    assert_eq!(
        regions.apply_prepared_goal_tick(second_phase, resolved),
        Err(RegionEntityStoreError::StalePhase)
    );
    assert_eq!(
        regions.motion_state(id).expect("unchanged motion").velocity,
        Vec3::ZERO
    );
}

#[test]
fn regional_goal_prepare_rejects_work_after_lane_acknowledgement() {
    let mut regions = RegionalEntityStore::new();
    let lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("region");
    let phase = regions.begin_phase().expect("phase");
    let id = regions
        .spawn(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    regions
        .acknowledge_lane(phase, 0)
        .expect("acknowledge lane");

    assert!(matches!(
        regions.prepare_goal_tick_with_pathing_for_ids(phase, 19, &HashSet::from([id])),
        Err(RegionEntityStoreError::Ownership(
            RegionOwnershipError::LaneCompleted
        ))
    ));
}

#[test]
fn regional_goal_apply_rejects_work_after_lane_acknowledgement() {
    let mut regions = RegionalEntityStore::new();
    let lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("region");
    let phase = regions.begin_phase().expect("phase");
    let mut entity = cow(Vec3::new(0.5, 64.0, 0.5));
    entity.goal = GoalState::FollowPosition {
        target: Vec3::new(2.5, 64.0, 0.5),
        speed: 0.25,
    };
    let id = regions.spawn(phase, lease, entity).expect("cow");
    let resolved = regions
        .prepare_goal_tick_with_pathing_for_ids(phase, 20, &HashSet::from([id]))
        .expect("prepare goals")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    regions
        .acknowledge_lane(phase, 0)
        .expect("acknowledge lane");

    assert_eq!(
        regions.apply_prepared_goal_tick(phase, resolved),
        Err(RegionEntityStoreError::Ownership(
            RegionOwnershipError::LaneCompleted
        ))
    );
    assert_eq!(
        regions.motion_state(id).expect("unchanged motion").velocity,
        Vec3::ZERO
    );
}

#[test]
fn regional_goal_apply_rejects_a_batch_from_another_store() {
    let mut source = RegionalEntityStore::new();
    let source_lease = source
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("source region");
    let source_phase = source.begin_phase().expect("source phase");
    let mut source_entity = cow(Vec3::new(0.5, 64.0, 0.5));
    source_entity.goal = GoalState::FollowPosition {
        target: Vec3::new(2.5, 64.0, 0.5),
        speed: 0.25,
    };
    let source_id = source
        .spawn(source_phase, source_lease, source_entity)
        .expect("source cow");
    let resolved = source
        .prepare_goal_tick_with_pathing_for_ids(source_phase, 21, &HashSet::from([source_id]))
        .expect("prepare source goals")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    let mut target = RegionalEntityStore::new();
    let target_lease = target
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("target region");
    let target_phase = target.begin_phase().expect("target phase");
    let mut target_entity = cow(Vec3::new(0.5, 64.0, 0.5));
    target_entity.goal = GoalState::FollowPosition {
        target: Vec3::new(2.5, 64.0, 0.5),
        speed: 0.25,
    };
    let target_id = target
        .spawn(target_phase, target_lease, target_entity)
        .expect("target cow");
    assert_eq!(source_id, target_id);

    assert_eq!(
        target.apply_prepared_goal_tick(target_phase, resolved),
        Err(RegionEntityStoreError::StaleLease)
    );
    assert_eq!(
        target
            .motion_state(target_id)
            .expect("unchanged target motion")
            .velocity,
        Vec3::ZERO
    );
}

#[test]
fn production_authority_rejects_non_finite_kinematics_without_panicking() {
    let mut authority = RegionalEntityAuthority::default();
    let id = authority.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    let before = authority
        .snapshot(id)
        .expect("entity before invalid motion");
    let mut invalid = movement(id, before.position);
    invalid.position.x = f64::NAN;

    assert_eq!(authority.apply_kinematics([invalid]), 0);
    assert_eq!(authority.snapshot(id), Some(before));
}

#[test]
fn conditional_animal_batch_rejects_stale_snapshot_without_partial_mutation() {
    let mut authority = RegionalEntityAuthority::default();
    let ready = AnimalBreedingState {
        age_ticks: 0,
        love_ticks: 20,
        sheep_wool: None,
    };
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.animal = Some(ready);
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.animal = Some(ready);
    let ids = authority.spawn_batch([west, east]);
    let expected = ids
        .iter()
        .map(|id| authority.snapshot(*id).expect("parent snapshot"))
        .collect::<Vec<_>>();
    assert!(authority.set_velocity(ids[0], Vec3::new(0.25, 0.0, 0.0)));

    let cooldown = AnimalBreedingState {
        age_ticks: 6_000,
        love_ticks: 0,
        sheep_wool: None,
    };
    assert!(!authority.set_animal_states_if_current([
        (expected[0].clone(), cooldown),
        (expected[1].clone(), cooldown),
    ]));
    assert_eq!(
        authority.snapshot(ids[0]).and_then(|entity| entity.animal),
        Some(ready)
    );
    assert_eq!(
        authority.snapshot(ids[1]).and_then(|entity| entity.animal),
        Some(ready)
    );
}

#[test]
fn conditional_snapshot_batch_rejects_stale_member_without_partial_mutation() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let authority = runtime.handle();
    let ids = authority
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("entities");
    let expected = ids
        .iter()
        .map(|id| {
            authority
                .snapshot(*id)
                .expect("snapshot request")
                .expect("entity snapshot")
        })
        .collect::<Vec<_>>();
    authority
        .set_velocities([(ids[1], Vec3::new(0.25, 0.0, 0.0))])
        .expect("stale second snapshot");
    let actual_after_stale = ids
        .iter()
        .map(|id| {
            authority
                .snapshot(*id)
                .expect("snapshot request")
                .expect("current snapshot")
        })
        .collect::<Vec<_>>();
    let mut stale_next = expected.clone();
    stale_next[0].health -= 1.0;
    stale_next[1].health -= 1.0;

    assert_eq!(
        authority.replace_snapshots_if_current(expected.into_iter().zip(stale_next)),
        Ok(false)
    );
    assert_eq!(
        ids.iter()
            .map(|id| {
                authority
                    .snapshot(*id)
                    .expect("snapshot request")
                    .expect("unchanged snapshot")
            })
            .collect::<Vec<_>>(),
        actual_after_stale
    );

    let current = actual_after_stale;
    let mut next = current.clone();
    next[0].health -= 1.0;
    next[1].health -= 1.0;
    assert_eq!(
        authority.replace_snapshots_if_current(current.into_iter().zip(next.clone())),
        Ok(true)
    );
    assert_eq!(
        ids.iter()
            .map(|id| {
                authority
                    .snapshot(*id)
                    .expect("snapshot request")
                    .expect("committed snapshot")
            })
            .collect::<Vec<_>>(),
        next
    );
    runtime.shutdown().expect("clean owner shutdown");
}

#[test]
fn owner_coordinator_moves_physical_stores_to_lanes_and_round_trips_them() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let west = regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");

    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
    assert_eq!(coordinator.lane_count(), 2);
    coordinator
        .set_velocities([
            (east, Vec3::new(0.2, 0.0, 0.0)),
            (west, Vec3::new(0.1, 0.0, 0.0)),
        ])
        .expect("cross-lane velocity phase");
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .velocity,
        Vec3::new(0.1, 0.0, 0.0)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .velocity,
        Vec3::new(0.2, 0.0, 0.0)
    );

    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert_eq!(restored.region_for(west), Some(RegionKey::new(0, 0)));
    assert_eq!(restored.region_for(east), Some(RegionKey::new(1, 0)));
    assert_eq!(restored.len(), 2);
    assert_eq!(
        restored.snapshot(west).expect("restored west").velocity,
        Vec3::new(0.1, 0.0, 0.0)
    );
    assert_eq!(
        restored.snapshot(east).expect("restored east").velocity,
        Vec3::new(0.2, 0.0, 0.0)
    );
}

#[test]
fn owner_coordinator_excludes_idle_lanes_from_local_mutation_phase() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let west = regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");

    coordinator
        .set_velocities([(west, Vec3::new(0.1, 0.0, 0.0))])
        .expect("west-only velocity phase");

    assert_eq!(coordinator.lanes[&0].prepare_request_count(), 1);
    assert_eq!(coordinator.lanes[&0].prepare_and_commit_request_count(), 1);
    assert_eq!(coordinator.lanes[&1].prepare_request_count(), 0);
}

#[test]
fn owner_standalone_local_kinematics_does_not_reread_snapshots() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let west = regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let west_second = regions
        .spawn(phase, west_lease, cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("second west cow");
    regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
    let expected = coordinator
        .snapshot(west)
        .expect("west read")
        .expect("west snapshot");
    let expected_second = coordinator
        .snapshot(west_second)
        .expect("second west read")
        .expect("second west snapshot");
    for lane in coordinator.lanes.values() {
        lane.reset_snapshot_batch_request_count();
    }
    let target = movement(west, Vec3::new(0.75, 64.0, 0.5));
    let target_second = movement(west_second, Vec3::new(1.75, 64.0, 0.5));
    let sequence_before = coordinator.commit_state.sequence_watermark();

    assert!(
        coordinator
            .apply_kinematics_if_current([(expected, target), (expected_second, target_second),])
            .expect("west kinematics")
    );

    assert_eq!(
        coordinator.commit_state.sequence_watermark(),
        sequence_before + 1
    );
    assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
}

#[test]
fn owner_plain_batch_spawn_does_not_scan_existing_lanes() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
    for lane in coordinator.lanes.values() {
        lane.reset_snapshot_batch_request_count();
    }

    let spawned = coordinator
        .spawn_batch([
            cow(Vec3::new(1.5, 64.0, 0.5)),
            cow(Vec3::new(2.5, 64.0, 0.5)),
        ])
        .expect("plain west batch");

    assert_eq!(spawned.len(), 2);
    assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
}

#[test]
fn owner_coordinator_rolls_back_when_commit_decision_is_not_durable() {
    let mut regions = RegionalEntityStore::new();
    let lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("region");
    let phase = regions.begin_phase().expect("spawn phase");
    let entity = regions
        .spawn(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let journal = Arc::new(Mutex::new(TestDecisionJournalState {
        fail_record: true,
        ..TestDecisionJournalState::default()
    }));
    let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
        regions,
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner coordinator");
    assert_eq!(
        coordinator.set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))]),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        coordinator
            .snapshot(entity)
            .expect("entity read")
            .expect("entity snapshot")
            .velocity,
        Vec3::ZERO
    );
    assert!(journal.lock().expect("journal state").cleared.is_empty());
    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn owner_coordinator_fail_stops_when_journal_outcome_is_unknown() {
    let mut regions = RegionalEntityStore::new();
    let lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("region");
    let phase = regions.begin_phase().expect("spawn phase");
    let entity = regions
        .spawn(phase, lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let journal = Arc::new(Mutex::new(TestDecisionJournalState {
        fail_record_unknown: true,
        ..TestDecisionJournalState::default()
    }));
    let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
        regions,
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner coordinator");

    assert_eq!(
        coordinator.set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))]),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        coordinator.snapshot(entity),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(
        coordinator.set_velocities([(entity, Vec3::new(0.5, 0.0, 0.0))]),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(
        coordinator.snapshots(),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(
        coordinator.save_barrier(),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(journal.lock().expect("journal state").commits.len(), 1);
}

#[test]
fn owner_coordinator_clears_durable_decision_only_after_saved_snapshot_checkpoint() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner coordinator");
    let west = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");

    coordinator
        .set_velocities([
            (west, Vec3::new(0.1, 0.0, 0.0)),
            (east, Vec3::new(0.2, 0.0, 0.0)),
        ])
        .expect("journaled velocity phase");

    let state = journal.lock().expect("journal state");
    let decision = state.commits.last().expect("recorded decision").clone();
    assert_eq!(decision.sequence_watermark(), 4);
    assert_eq!(
        decision
            .upserts()
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        vec![west, east]
    );
    assert!(decision.removed().is_empty());
    assert!(
        state.cleared.is_empty(),
        "lane finalize is not a durable entity snapshot"
    );
    drop(state);

    let saved = coordinator.save_barrier().expect("owner save barrier");
    assert!(saved.journal_phases().contains(&decision.phase()));
    let saved_phases = saved.journal_phases().to_vec();
    coordinator
        .clear_recovered_commits(saved_phases.iter().copied())
        .expect("durable entity snapshot checkpoints journal");
    assert_eq!(journal.lock().expect("journal state").cleared, saved_phases);
    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn owner_coordinator_rebalances_regions_without_losing_entities() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let west = regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");

    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 1).expect("owner coordinator");
    let stale_east = coordinator
        .ownership
        .lease(RegionKey::new(1, 0))
        .expect("east lease before scale-up");

    assert_eq!(coordinator.reconfigure_lanes(2), Ok(2));
    assert_eq!(coordinator.lane_count(), 2);
    let current_east = coordinator
        .ownership
        .lease(RegionKey::new(1, 0))
        .expect("east lease after scale-up");
    assert_ne!(current_east, stale_east);
    assert_eq!(current_east.lane, 1);
    assert_eq!(
        coordinator
            .lanes
            .get(&stale_east.lane)
            .expect("old lane")
            .snapshot(stale_east, east),
        Err(super::RegionOwnerLaneError::UnknownRegion)
    );
    assert_eq!(
        coordinator.snapshot(west).expect("west read").unwrap().id,
        west
    );
    assert_eq!(
        coordinator.snapshot(east).expect("east read").unwrap().id,
        east
    );

    assert_eq!(coordinator.reconfigure_lanes(1), Ok(1));
    assert_eq!(coordinator.lane_count(), 1);
    assert_eq!(coordinator.snapshots().expect("snapshots").len(), 2);
    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert_eq!(restored.len(), 2);
}

#[test]
fn owner_coordinator_aborts_ready_lane_when_peer_rejects_prepare() {
    let mut regions = RegionalEntityStore::new();
    let west_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("west region");
    let east_lease = regions
        .assign_region(RegionKey::new(1, 0), 0)
        .expect("east region");
    let phase = regions.begin_phase().expect("spawn phase");
    let west = regions
        .spawn(phase, west_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = regions
        .spawn(phase, east_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    regions.acknowledge_lane(phase, 0).expect("spawn lane");
    regions.finish_phase(phase).expect("spawn phase complete");
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 2).expect("owner coordinator");
    let east_key = RegionKey::new(1, 0);
    let current = coordinator
        .ownership
        .lease(east_key)
        .expect("east coordinator lease");
    coordinator
        .ownership
        .reassign(current, current.lane)
        .expect("coordinator-only stale epoch");

    assert_eq!(
        coordinator.set_velocities([
            (west, Vec3::new(0.1, 0.0, 0.0)),
            (east, Vec3::new(0.2, 0.0, 0.0)),
        ]),
        Err(super::RegionOwnerLaneError::StaleLease)
    );
    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert_eq!(restored.snapshot(west).expect("west").velocity, Vec3::ZERO);
    assert_eq!(restored.snapshot(east).expect("east").velocity, Vec3::ZERO);
}

#[test]
fn owner_coordinator_spawns_into_an_empty_world_and_balances_new_regions() {
    let empty = RegionalEntityStore::new();
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(empty, 2).expect("empty owner coordinator");
    assert_eq!(coordinator.lane_count(), 2);

    let west = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");

    assert_eq!(west, EntityId(1));
    assert_eq!(east, EntityId(2));
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .position,
        Vec3::new(0.5, 64.0, 0.5)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );
    assert_ne!(
        coordinator
            .ownership
            .lease(RegionKey::new(0, 0))
            .expect("west lease")
            .lane,
        coordinator
            .ownership
            .lease(RegionKey::new(1, 0))
            .expect("east lease")
            .lane
    );
    assert_eq!(
        coordinator
            .snapshots()
            .expect("coordinator snapshot batch")
            .into_iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        vec![west, east]
    );

    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert_eq!(restored.len(), 2);
    assert_eq!(restored.region_for(west), Some(RegionKey::new(0, 0)));
    assert_eq!(restored.region_for(east), Some(RegionKey::new(1, 0)));
}

#[test]
fn owner_coordinator_removes_spawned_entity_without_leaving_global_indexes() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("spawn cow");

    let removed = coordinator
        .remove(entity)
        .expect("remove cow")
        .expect("removed snapshot");

    assert_eq!(removed.id, entity);
    assert_eq!(
        coordinator.snapshot(entity).expect("read after remove"),
        None
    );
    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert!(!restored.contains(entity));
    assert!(!restored.contains_uuid(removed.uuid));
}

#[test]
fn owner_coordinator_simulation_projection_skips_removed_requested_ids() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let kept = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("spawn kept cow");
    let removed = coordinator
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("spawn removed cow");
    coordinator.remove(removed).expect("remove cow");

    let projections = coordinator
        .simulation_projections_for_ids(&HashSet::from([kept, removed]))
        .expect("read surviving simulation projection");

    assert_eq!(
        projections
            .into_iter()
            .map(|projection| projection.id)
            .collect::<Vec<_>>(),
        vec![kept]
    );
}

#[test]
fn owner_coordinator_rejects_cross_lane_animal_cas_when_one_parent_is_stale() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let ready = AnimalBreedingState {
        age_ticks: 0,
        love_ticks: 20,
        sheep_wool: None,
    };
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.animal = Some(ready);
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.animal = Some(ready);
    let west = coordinator.spawn(west).expect("west cow");
    let east = coordinator.spawn(east).expect("east cow");
    let expected_west = coordinator
        .snapshot(west)
        .expect("west read")
        .expect("west snapshot");
    let expected_east = coordinator
        .snapshot(east)
        .expect("east read")
        .expect("east snapshot");
    coordinator
        .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
        .expect("make east stale");
    let cooldown = AnimalBreedingState {
        age_ticks: 6_000,
        love_ticks: 0,
        sheep_wool: None,
    };

    assert!(
        !coordinator
            .set_animal_states_if_current([(expected_west, cooldown), (expected_east, cooldown),])
            .expect("rejected CAS")
    );
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .animal,
        Some(ready)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .animal,
        Some(ready)
    );
    let current_west = coordinator
        .snapshot(west)
        .expect("west read")
        .expect("current west snapshot");
    let current_east = coordinator
        .snapshot(east)
        .expect("east read")
        .expect("current east snapshot");
    assert!(
        coordinator
            .set_animal_states_if_current([(current_west, cooldown), (current_east, cooldown),])
            .expect("accepted CAS")
    );
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .animal,
        Some(cooldown)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .animal,
        Some(cooldown)
    );
}

#[test]
fn owner_coordinator_rejects_cross_lane_kinematics_when_one_input_is_stale() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let west = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let expected_west = coordinator
        .snapshot(west)
        .expect("west read")
        .expect("west snapshot");
    let expected_east = coordinator
        .snapshot(east)
        .expect("east read")
        .expect("east snapshot");
    coordinator
        .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
        .expect("make east stale");

    assert!(
        !coordinator
            .apply_kinematics_if_current([
                (expected_west, movement(west, Vec3::new(1.5, 64.0, 0.5))),
                (expected_east, movement(east, Vec3::new(129.5, 64.0, 0.5)),),
            ])
            .expect("rejected kinematics")
    );
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .position,
        Vec3::new(0.5, 64.0, 0.5)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .velocity,
        Vec3::new(0.25, 0.0, 0.0)
    );

    let current_west = coordinator
        .snapshot(west)
        .expect("west read")
        .expect("current west snapshot");
    let current_east = coordinator
        .snapshot(east)
        .expect("east read")
        .expect("current east snapshot");
    assert!(
        coordinator
            .apply_kinematics_if_current([
                (current_west, movement(west, Vec3::new(1.5, 64.0, 0.5))),
                (current_east, movement(east, Vec3::new(129.5, 64.0, 0.5)),),
            ])
            .expect("accepted kinematics")
    );
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .position,
        Vec3::new(1.5, 64.0, 0.5)
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .position,
        Vec3::new(129.5, 64.0, 0.5)
    );
}

#[test]
fn owner_coordinator_rejects_kinematics_batch_when_one_input_is_missing() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("existing cow");
    let expected = coordinator
        .snapshot(entity)
        .expect("cow read")
        .expect("cow snapshot");
    let missing_id = EntityId(entity.0 + 1_000);
    let mut missing = expected.clone();
    missing.id = missing_id;
    missing.position = Vec3::new(128.5, 64.0, 0.5);

    assert!(
        !coordinator
            .apply_kinematics_if_current([
                (
                    expected.clone(),
                    movement(entity, Vec3::new(1.5, 64.0, 0.5)),
                ),
                (missing, movement(missing_id, Vec3::new(129.5, 64.0, 0.5)),),
            ])
            .expect("missing input is a rejected compare")
    );
    assert_eq!(
        coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("cow remains")
            .position,
        expected.position
    );
}

#[test]
fn owner_coordinator_moves_standalone_entity_across_region_owners() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    let expected = coordinator
        .snapshot(entity)
        .expect("cow read")
        .expect("cow snapshot");
    let target = movement(entity, Vec3::new(128.5, 64.0, 0.5));
    coordinator
        .set_velocities([(entity, Vec3::new(0.25, 0.0, 0.0))])
        .expect("make source stale");

    assert!(
        !coordinator
            .apply_kinematics_if_current([(expected, target)])
            .expect("reject stale boundary kinematics")
    );
    assert_eq!(
        coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("source cow")
            .position,
        Vec3::new(127.5, 64.0, 0.5)
    );
    assert_eq!(coordinator.locations[&entity], RegionKey::new(0, 0));
    let expected = coordinator
        .snapshot(entity)
        .expect("cow read")
        .expect("fresh cow snapshot");

    assert!(
        coordinator
            .apply_kinematics_if_current([(expected, target)])
            .expect("boundary kinematics")
    );
    assert_eq!(
        coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("migrated cow")
            .position,
        target.position
    );
    assert_eq!(
        coordinator
            .ownership
            .lease(RegionKey::new(0, 0))
            .expect("source lease")
            .lane,
        0
    );
    assert_eq!(
        coordinator
            .ownership
            .lease(RegionKey::new(1, 0))
            .expect("target lease")
            .lane,
        1
    );

    let restored = coordinator.shutdown().expect("coordinator shutdown");
    assert_eq!(restored.region_for(entity), Some(RegionKey::new(1, 0)));
    assert_eq!(
        restored.snapshot(entity).expect("restored cow").position,
        target.position
    );
}

#[test]
fn owner_coordinator_damage_uses_snapshot_cas_and_reports_lethal_result() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let expected = coordinator
        .snapshot(entity)
        .expect("cow read")
        .expect("cow snapshot");

    let damaged = coordinator
        .damage_if_current(expected.clone(), damage_request(5.0))
        .expect("damage")
        .expect("accepted damage");
    assert_eq!(damaged.snapshot.health, 15.0);
    assert!(!damaged.killed);
    assert!(
        coordinator
            .damage_if_current(expected, damage_request(5.0))
            .expect("stale damage")
            .is_none()
    );
    assert_eq!(
        coordinator
            .snapshot(entity)
            .expect("cow read")
            .expect("cow snapshot")
            .health,
        15.0
    );

    let current = coordinator
        .snapshot(entity)
        .expect("cow read")
        .expect("current cow snapshot");
    let killed = coordinator
        .damage_if_current(current, damage_request(20.0))
        .expect("lethal damage")
        .expect("accepted lethal damage");
    assert!(killed.killed);
    assert_eq!(killed.snapshot.health, 0.0);
    assert_eq!(
        killed.snapshot.lifecycle,
        crate::EntityLifecycle::Despawning
    );
}

#[test]
fn owner_coordinator_goal_apply_rejects_stale_lane_then_commits_fresh_batches() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.goal = GoalState::Wander {
        speed: 0.2,
        period_ticks: 20,
    };
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.goal = west.goal.clone();
    let west = coordinator.spawn(west).expect("west cow");
    let east = coordinator.spawn(east).expect("east cow");
    let active = HashSet::from([west, east]);
    let prepared = coordinator
        .prepare_goal_tick_with_pathing_for_ids(23, &active)
        .expect("prepare goals");
    coordinator
        .set_velocities([(east, Vec3::new(0.25, 0.0, 0.0))])
        .expect("make east stale");

    let rejected = coordinator
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("reject stale goals");
    assert_eq!(rejected, GoalTickStats::default());
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .velocity,
        Vec3::ZERO
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .velocity,
        Vec3::new(0.25, 0.0, 0.0)
    );

    let prepared = coordinator
        .prepare_goal_tick_with_pathing_for_ids(23, &active)
        .expect("prepare fresh goals");
    let applied = coordinator
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("apply fresh goals");
    assert_eq!(applied.decisions_applied, 2);
    assert_ne!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .velocity,
        Vec3::ZERO
    );
    assert_ne!(
        coordinator
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .velocity,
        Vec3::new(0.25, 0.0, 0.0)
    );
}

#[test]
fn owner_goal_apply_returns_sorted_simulation_results_without_post_apply_snapshots() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("owner coordinator");
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.goal = GoalState::Wander {
        speed: 0.2,
        period_ticks: 20,
    };
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.goal = west.goal.clone();
    let west = coordinator.spawn(west).expect("west cow");
    let east = coordinator.spawn(east).expect("east cow");
    let active = HashSet::from([east, west]);
    let resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids(23, &active)
        .expect("prepare goals")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    for lane in coordinator.lanes.values() {
        lane.reset_snapshot_batch_request_count();
    }

    let (stats, results, versioned) = coordinator
        .apply_prepared_goal_tick_and_kinematics_for_ids(resolved, &active, false)
        .expect("apply goals and project simulation results")
        .expect("fresh goals apply");

    assert_eq!(stats.decisions_applied, 2);
    assert_eq!(
        results
            .iter()
            .map(|result| result.physics.id)
            .collect::<Vec<_>>(),
        vec![west, east]
    );
    assert!(
        results
            .iter()
            .all(|result| result.physics.velocity != Vec3::ZERO)
    );
    assert!(versioned.is_some());
    assert!(
        coordinator
            .lanes
            .values()
            .all(|lane| lane.snapshot_batch_request_count() == 0),
        "typed post-apply projection must not materialize full snapshots"
    );

    let stale = coordinator
        .prepare_goal_tick_with_pathing_for_ids(24, &active)
        .expect("prepare stale goals")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let east_before_stale = coordinator
        .snapshot(east)
        .expect("east read before stale apply")
        .expect("east snapshot before stale apply");
    let current_velocity = Vec3::new(0.125, 0.25, 0.375);
    coordinator
        .set_velocities([(west, current_velocity)])
        .expect("make prepared goals stale");
    for lane in coordinator.lanes.values() {
        lane.reset_snapshot_batch_request_count();
    }
    let rejected = coordinator
        .apply_prepared_goal_tick_and_kinematics_for_ids(stale, &active, false)
        .expect("reject stale goals");

    assert!(rejected.is_none());
    assert_eq!(
        coordinator
            .snapshot(west)
            .expect("west read")
            .expect("west snapshot")
            .velocity,
        current_velocity
    );
    assert_eq!(
        coordinator
            .snapshot(east)
            .expect("east read after stale apply")
            .expect("east snapshot after stale apply"),
        east_before_stale,
        "a fresh lane committed during one-phase admission must roll back when its peer is stale"
    );
    assert!(
        coordinator
            .lanes
            .values()
            .all(|lane| lane.snapshot_batch_request_count() == 0),
        "stale typed projection must not materialize full snapshots"
    );
}

#[test]
fn owner_hostile_goal_selection_is_explicit_and_applies_atomically() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let original_target = Vec3::new(10.5, 64.0, 0.5);
    let mut zombie = SpawnEntity::new(54, "minecraft:zombie", Vec3::new(0.5, 64.0, 0.5));
    zombie.goal = GoalState::FollowPosition {
        target: original_target,
        speed: 0.05,
    };
    let zombie = handle.spawn(zombie).expect("zombie");
    let active = HashSet::from([zombie]);

    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(active.clone()))
        .expect("prepare without hostile selection");
    handle
        .apply_prepared_goal_tick_and_simulation_results_for_ids_deferred_journal(
            prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT),
            Arc::new(active.clone()),
        )
        .expect("apply without hostile selection")
        .expect("fresh default goal apply");
    assert_eq!(
        handle
            .snapshot(zombie)
            .expect("zombie read")
            .expect("zombie")
            .goal,
        GoalState::FollowPosition {
            target: original_target,
            speed: 0.05,
        },
        "default goal inputs must not reseat hostile targets"
    );

    let target = Vec3::new(3.5, 64.0, 0.5);
    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids_with_inputs(
            2,
            Arc::new(active.clone()),
            super::RegionalGoalTickInputs {
                hostile_target_positions: Some(Arc::from([target])),
                ..super::RegionalGoalTickInputs::default()
            },
        )
        .expect("prepare explicit hostile selection");
    let (_, results, _) = handle
        .apply_prepared_goal_tick_and_simulation_results_for_ids_deferred_journal(
            prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT),
            Arc::new(active.clone()),
        )
        .expect("apply explicit hostile selection")
        .expect("fresh explicit goal apply");
    assert_eq!(results.len(), 1);
    let GoalState::FollowPosition {
        target: selected_target,
        speed,
    } = handle
        .snapshot(zombie)
        .expect("zombie read")
        .expect("zombie")
        .goal
    else {
        panic!("explicit hostile selection must install a follow goal");
    };
    assert_eq!(selected_target, target);
    assert!((speed - 2.3).abs() < 1.0e-9);

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_goal_tick_reads_checkpoints_only_from_participant_lanes() {
    let mut regions = RegionalEntityStore::new();
    let follower_lease = regions
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("follower region");
    let target_lease = regions
        .assign_region(RegionKey::new(1, 0), 1)
        .expect("target region");
    let unrelated_lease = regions
        .assign_region(RegionKey::new(2, 0), 2)
        .expect("unrelated region");
    let phase = regions.begin_phase().expect("spawn phase");
    let follower = regions
        .spawn(phase, follower_lease, cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("follower");
    let target = regions
        .spawn(phase, target_lease, cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("target");
    regions
        .spawn(phase, unrelated_lease, cow(Vec3::new(256.5, 64.0, 0.5)))
        .expect("unrelated cow");
    regions.acknowledge_lane(phase, 0).expect("follower lane");
    regions.acknowledge_lane(phase, 1).expect("target lane");
    regions.acknowledge_lane(phase, 2).expect("unrelated lane");
    regions.finish_phase(phase).expect("spawn complete");
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(regions, 3).expect("owner coordinator");
    assert!(
        coordinator
            .set_goal(follower, GoalState::FollowTarget { target, speed: 0.2 },)
            .expect("set follower goal")
    );
    let cached_follower = coordinator
        .snapshots_for_ids(&HashSet::from([follower]))
        .expect("cache follower input");
    for lane in coordinator.lanes.values() {
        lane.reset_snapshot_batch_request_count();
        lane.reset_goal_checkpoint_batch_request_count();
    }

    let resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids_from_snapshots(
            23,
            &HashSet::from([follower]),
            Some((
                cached_follower,
                std::collections::BTreeMap::from([(
                    0,
                    coordinator.lanes[&0].reader().state_version(),
                )]),
            )),
            super::RegionalGoalTickInputs::default(),
        )
        .expect("prepare follower goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&2].snapshot_batch_request_count(), 0);
    assert_eq!(
        coordinator.lanes[&0].goal_checkpoint_batch_request_count(),
        0
    );
    assert_eq!(
        coordinator.lanes[&1].goal_checkpoint_batch_request_count(),
        1
    );
    assert_eq!(
        coordinator.lanes[&2].goal_checkpoint_batch_request_count(),
        0
    );

    let stats = coordinator
        .apply_prepared_goal_tick(resolved)
        .expect("apply follower goal");

    assert_eq!(stats.decisions_applied, 1);
    assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&1].snapshot_batch_request_count(), 0);
    assert_eq!(coordinator.lanes[&2].snapshot_batch_request_count(), 0);
    assert_eq!(
        coordinator.lanes[&0].goal_checkpoint_batch_request_count(),
        0
    );
    assert_eq!(
        coordinator.lanes[&1].goal_checkpoint_batch_request_count(),
        2
    );
    assert_eq!(
        coordinator.lanes[&2].goal_checkpoint_batch_request_count(),
        0
    );
}

#[test]
fn owner_damage_batch_applies_independent_cross_lane_hits() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("two-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("damage batch entities");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm cross-lane damage routes");
    let requests = snapshots.into_iter().map(|snapshot| {
        let amount = if snapshot.id == ids[0] { 5.0 } else { 20.0 };
        (snapshot, damage_request(amount))
    });

    let damages = handle
        .damage_batch_if_current(requests)
        .expect("cross-lane damage batch");

    assert_eq!(damages.len(), 2);
    assert_eq!(damages[0].snapshot.id, ids[0]);
    assert_eq!(damages[0].snapshot.health, 15.0);
    assert!(!damages[0].killed);
    assert_eq!(damages[1].snapshot.id, ids[1]);
    assert_eq!(damages[1].snapshot.health, 0.0);
    assert!(damages[1].killed);
    drop(handle);
    runtime.shutdown().expect("damage batch shutdown");
}

#[test]
fn owner_damage_batch_rejects_passenger() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("one-lane owner runtime");
    let handle = runtime.handle();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(0.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = handle
        .spawn_batch([boat, cow(Vec3::new(1.5, 64.0, 0.5))])
        .expect("vehicle and passenger batch");
    let passenger = ids[1];
    let expected = handle
        .snapshot(passenger)
        .expect("passenger snapshot read")
        .expect("passenger snapshot");

    assert!(
        handle
            .damage_batch_if_current([(expected.clone(), damage_request(5.0))])
            .expect("conditional passenger damage resolves")
            .is_empty(),
        "passenger damage must resolve as an unapplied conditional batch"
    );
    assert_eq!(
        handle
            .snapshot(passenger)
            .expect("passenger re-read")
            .expect("passenger remains")
            .health,
        expected.health
    );
    drop(handle);
    runtime.shutdown().expect("passenger damage batch shutdown");
}

#[test]
fn owner_damage_batch_rejects_duplicate_targets() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("one-lane owner runtime");
    let handle = runtime.handle();
    let id = handle
        .spawn_batch([cow(Vec3::new(0.5, 64.0, 0.5))])
        .expect("damage batch entity")[0];
    let expected = handle.snapshot(id).expect("snapshot read").expect("entity");

    assert_eq!(
        handle.damage_batch_if_current([
            (expected.clone(), damage_request(1.0)),
            (expected, damage_request(1.0)),
        ]),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );
    drop(handle);
    runtime.shutdown().expect("duplicate damage batch shutdown");
}

#[test]
fn owner_goal_apply_handles_mutual_cross_lane_follow_targets_without_readmission_deadlock() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("two-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("mutual target entities");
    assert_eq!(
        handle
            .set_goals_deferred_journal([
                (
                    ids[0],
                    GoalState::FollowTarget {
                        target: ids[1],
                        speed: 0.2,
                    },
                ),
                (
                    ids[1],
                    GoalState::FollowTarget {
                        target: ids[0],
                        speed: 0.2,
                    },
                ),
            ])
            .expect("mutual target goals"),
        2
    );
    let active = HashSet::from([ids[0], ids[1]]);
    let resolved = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(active.clone()))
        .expect("prepare mutual target goals")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let stats = handle
        .apply_prepared_goal_tick(resolved)
        .expect("apply mutual target goals");
    assert_eq!(stats.decisions_applied, 2);
    assert_eq!(stats.missing_follow_targets, 0);
    assert!(
        handle
            .snapshots_for_ids(&active)
            .expect("mutual target snapshots")
            .iter()
            .all(|snapshot| snapshot.velocity != Vec3::ZERO)
    );
    drop(handle);
    runtime.shutdown().expect("mutual target shutdown");
}

#[test]
fn owner_goal_apply_lets_lane_reject_stale_local_input_without_resnapshot() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("owner coordinator");
    let follower = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("follower");
    assert!(
        coordinator
            .set_goal(
                follower,
                GoalState::FollowPosition {
                    target: Vec3::new(4.5, 64.0, 0.5),
                    speed: 0.2,
                },
            )
            .expect("set follower goal")
    );
    let resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids(23, &HashSet::from([follower]))
        .expect("prepare follower goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    assert!(
        coordinator
            .set_position(follower, Vec3::new(1.5, 64.0, 0.5))
            .expect("move follower")
    );
    coordinator.lanes[&0].reset_snapshot_batch_request_count();

    let stats = coordinator
        .apply_prepared_goal_tick(resolved)
        .expect("reject stale follower goal");

    assert_eq!(stats, GoalTickStats::default());
    assert_eq!(coordinator.lanes[&0].snapshot_batch_request_count(), 0);
    assert_eq!(
        coordinator
            .snapshot(follower)
            .expect("follower read")
            .expect("follower snapshot")
            .position,
        Vec3::new(1.5, 64.0, 0.5)
    );
}

#[test]
fn owner_goal_tick_rejects_a_missing_target_replaced_before_apply() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("empty owner coordinator");
    let target = coordinator
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("target");
    let follower = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("follower");
    coordinator
        .spawn(cow(Vec3::new(3.5, 64.0, 0.5)))
        .expect("unrelated cow");
    assert!(
        coordinator
            .set_goal(follower, GoalState::FollowTarget { target, speed: 0.2 },)
            .expect("set follower goal")
    );
    let removed = coordinator
        .remove(target)
        .expect("remove target")
        .expect("target snapshot");
    let follower_before = coordinator
        .snapshot(follower)
        .expect("follower read")
        .expect("follower snapshot");
    let resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids(23, &HashSet::from([follower]))
        .expect("prepare missing target goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let mut replacement = removed;
    replacement.uuid = Uuid::from_u128(23);
    replacement.position = Vec3::new(2.5, 64.0, 0.5);
    assert_eq!(
        coordinator
            .insert_snapshots_batch([replacement])
            .expect("restore replacement"),
        1
    );

    let stats = coordinator
        .apply_prepared_goal_tick(resolved)
        .expect("reject replacement target");

    assert_eq!(stats, GoalTickStats::default());
    assert_eq!(
        coordinator
            .snapshot(follower)
            .expect("follower after apply"),
        Some(follower_before)
    );
}

#[test]
fn owner_save_barrier_captures_one_finalized_sequence_and_stable_snapshots() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("empty owner coordinator");
    let west = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    coordinator
        .set_velocities([
            (west, Vec3::new(0.1, 0.0, 0.0)),
            (east, Vec3::new(0.2, 0.0, 0.0)),
        ])
        .expect("finalized motion");

    let saved = coordinator.save_barrier().expect("save barrier");
    assert!(saved.sequence_watermark() > 0);
    assert_eq!(
        saved
            .snapshots()
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        vec![west, east]
    );
    assert_eq!(saved.snapshots()[0].velocity, Vec3::new(0.1, 0.0, 0.0));
    assert_eq!(saved.snapshots()[1].velocity, Vec3::new(0.2, 0.0, 0.0));

    coordinator
        .set_velocities([(west, Vec3::new(0.3, 0.0, 0.0))])
        .expect("later motion");
    assert_eq!(saved.snapshots()[0].velocity, Vec3::new(0.1, 0.0, 0.0));
    let newer = coordinator.save_barrier().expect("newer save barrier");
    assert!(newer.sequence_watermark() > saved.sequence_watermark());
    assert_eq!(newer.snapshots()[0].velocity, Vec3::new(0.3, 0.0, 0.0));
}

#[test]
fn checkpoint_does_not_acknowledge_a_durable_mutation_appended_after_its_snapshot() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut spawn = cow(Vec3::new(0.5, 64.0, 0.5));
    spawn.animal = Some(AnimalBreedingState::adult());
    let entity = handle.spawn(spawn).expect("durable cow spawn");
    let initial = handle.save_barrier().expect("initial save barrier");
    handle
        .clear_recovered_commits(initial.journal_phases().iter().copied())
        .expect("clear initial journal phase");
    journal.lock().expect("journal state").cleared.clear();
    let mut later_snapshot = handle
        .snapshot(entity)
        .expect("current entity snapshot")
        .expect("spawned cow");
    later_snapshot.goal = GoalState::FollowPosition {
        target: Vec3::new(4.5, 64.0, 0.5),
        speed: 0.25,
    };

    let (snapshot_entered, snapshot_entered_rx) = mpsc::sync_channel(0);
    let (snapshot_release, snapshot_release_rx) = mpsc::sync_channel(0);
    handle
        .commit_state
        .pause_before_save_barrier_phase_snapshot(snapshot_entered, snapshot_release_rx);

    let checkpoint_handle = handle.clone();
    let (checkpoint_done, checkpoint_done_rx) = mpsc::channel();
    let checkpoint_thread = std::thread::spawn(move || {
        checkpoint_done
            .send(checkpoint_handle.save_barrier())
            .expect("publish checkpoint result");
    });
    snapshot_entered_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("checkpoint captured entity snapshot before journal phases");

    let appended_phase = super::RegionPhase(u64::MAX - 1);
    handle
        .commit_state
        .record_commit(
            &super::RegionalCommitDecision::from_parts(
                appended_phase,
                u64::MAX,
                vec![later_snapshot],
                Vec::new(),
            )
            .expect("valid later decision"),
        )
        .expect("append later durable decision");

    snapshot_release
        .send(())
        .expect("release checkpoint phase snapshot");
    let checkpoint = checkpoint_done_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("checkpoint result")
        .expect("checkpoint succeeds");
    checkpoint_thread.join().expect("join checkpoint");

    assert_eq!(
        checkpoint.snapshots()[0].goal,
        GoalState::Idle,
        "the checkpoint snapshot predates the concurrent durable decision"
    );
    assert!(
        !checkpoint.journal_phases().contains(&appended_phase),
        "a checkpoint must not acknowledge a journal decision absent from its entity snapshot"
    );
    handle
        .clear_recovered_commits(checkpoint.journal_phases().iter().copied())
        .expect("acknowledge checkpoint journal phases");
    assert!(
        !journal
            .lock()
            .expect("journal state")
            .cleared
            .contains(&appended_phase),
        "compaction must retain the mutation for a later checkpoint"
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_restore_batch_preserves_local_vehicle_graph_and_rejects_cross_region_graph() {
    let mut source = RegionalEntityStore::new();
    let west_lease = source
        .assign_region(RegionKey::new(0, 0), 0)
        .expect("source region");
    let phase = source.begin_phase().expect("source phase");
    let passenger = source
        .spawn(phase, west_lease, cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("passenger");
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(0.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(passenger);
    source.spawn(phase, west_lease, boat).expect("boat");
    let snapshots = source.snapshots().collect::<Vec<_>>();

    let mut restored = super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
        .expect("restore coordinator");
    assert_eq!(restored.insert_snapshots_batch(snapshots.clone()), Ok(2));
    assert_eq!(
        restored
            .save_barrier()
            .expect("restored barrier")
            .into_snapshots(),
        snapshots
    );

    let mut invalid = snapshots;
    invalid
        .iter_mut()
        .find(|snapshot| snapshot.vehicle.is_some())
        .expect("boat snapshot")
        .position = Vec3::new(128.5, 64.0, 0.5);
    let mut rejected = super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
        .expect("reject coordinator");
    assert_eq!(
        rejected.insert_snapshots_batch(invalid),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );
    assert!(
        rejected
            .save_barrier()
            .expect("empty barrier")
            .snapshots()
            .is_empty()
    );
}

#[test]
fn full_snapshot_cas_updates_vehicle_indexes_and_rejects_invalid_graphs() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("owner coordinator");
    let passenger = coordinator
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("passenger");
    let boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        8,
        "minecraft:oak_boat",
        Vec3::new(0.5, 64.0, 0.5),
    );
    let boat = coordinator.spawn(boat).expect("boat");
    let detached = coordinator.snapshot(boat).unwrap().unwrap();
    let mut attached = detached.clone();
    attached.vehicle.as_mut().unwrap().passenger = Some(passenger);
    assert!(
        coordinator
            .replace_snapshot_if_current(detached.clone(), attached.clone())
            .unwrap()
    );
    assert_eq!(coordinator.vehicle_passengers.get(&boat), Some(&passenger));
    assert_eq!(coordinator.passenger_vehicles.get(&passenger), Some(&boat));

    let mut reattached = attached.clone();
    reattached.vehicle.as_mut().unwrap().passenger = None;
    assert!(
        coordinator
            .replace_snapshot_if_current(attached.clone(), reattached.clone())
            .unwrap()
    );
    assert!(!coordinator.vehicle_passengers.contains_key(&boat));
    assert!(!coordinator.passenger_vehicles.contains_key(&passenger));

    let stale_expected = attached;
    let mut stale_reattach = reattached.clone();
    stale_reattach.vehicle.as_mut().unwrap().passenger = Some(passenger);
    assert!(
        !coordinator
            .replace_snapshot_if_current(stale_expected, stale_reattach)
            .unwrap()
    );
    assert!(!coordinator.vehicle_passengers.contains_key(&boat));

    let mut dangling = reattached.clone();
    dangling.vehicle.as_mut().unwrap().passenger = Some(EntityId(i32::MAX));
    assert_eq!(
        coordinator.replace_snapshot_if_current(reattached, dangling),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );
    assert!(!coordinator.vehicle_passengers.contains_key(&boat));
    assert!(!coordinator.passenger_vehicles.contains_key(&passenger));
}

mod vehicle_transfer_tests;

#[test]
fn owner_runtime_routes_exact_commands_and_returns_state_on_shutdown() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    handle
        .set_velocities([
            (west, Vec3::new(0.1, 0.0, 0.0)),
            (east, Vec3::new(0.2, 0.0, 0.0)),
        ])
        .expect("owner velocities");

    let saved = handle.save_barrier().expect("runtime save barrier");
    assert_eq!(saved.snapshots().len(), 2);
    assert_eq!(
        handle
            .snapshot(east)
            .expect("east read")
            .expect("east snapshot")
            .velocity,
        Vec3::new(0.2, 0.0, 0.0)
    );
    drop(handle);

    let recovered = runtime.shutdown().expect("runtime shutdown");
    assert_eq!(recovered.len(), 2);
    assert_eq!(
        recovered.snapshot(west).expect("recovered west").velocity,
        Vec3::new(0.1, 0.0, 0.0)
    );
}

#[test]
fn journal_checkpoint_does_not_block_checkpoint_only_goal_updates() {
    let state = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let (clear_started, clear_started_rx) = mpsc::sync_channel(0);
    let (clear_release, clear_release_rx) = mpsc::sync_channel(0);
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(BlockingClearDecisionJournal {
            state,
            clear_started,
            clear_release: clear_release_rx,
        }),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("durable cow spawn");
    let phases = handle
        .save_barrier()
        .expect("save barrier")
        .journal_phases()
        .to_vec();
    assert!(!phases.is_empty());

    let clear_handle = handle.clone();
    let clear = std::thread::spawn(move || clear_handle.clear_recovered_commits(phases));
    clear_started_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("journal clear starts");

    let goal_handle = handle.clone();
    let (goal_done, goal_done_rx) = mpsc::channel();
    let goal = std::thread::spawn(move || {
        let result = goal_handle.set_goals_deferred_journal([(
            entity,
            GoalState::FollowPosition {
                target: Vec3::new(4.5, 64.0, 0.5),
                speed: 0.25,
            },
        )]);
        goal_done.send(result).expect("publish goal update");
    });

    let goal_before_clear_finished = goal_done_rx.recv_timeout(Duration::from_secs(1));
    clear_release.send(()).expect("release journal clear");
    assert_eq!(clear.join().expect("join journal clear"), Ok(()));
    goal.join().expect("join goal update");
    assert_eq!(
        goal_before_clear_finished
            .expect("checkpoint-only goal update must not wait for journal checkpoint I/O"),
        Ok(1)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_selected_reads_bypass_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let selected = HashSet::from([west, east]);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("warm direct routes")
            .len(),
        2
    );

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (read_complete, read_complete_rx) = mpsc::channel();
    let read_handle = handle.clone();
    let reader = std::thread::spawn(move || {
        let result = read_handle.snapshots_for_ids(&selected);
        read_complete.send(result).expect("publish direct read");
    });
    let snapshots = read_complete_rx
        .recv_timeout(Duration::from_secs(1))
        .expect("direct owner-lane read must not wait for coordinator")
        .expect("direct owner-lane read");
    assert_eq!(
        snapshots
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<HashSet<_>>(),
        HashSet::from([west, east])
    );

    coordinator_release.send(()).expect("release coordinator");
    reader.join().expect("join direct reader");
    handle.status().expect("coordinator resumed");
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_point_read_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("warm point route")
            .expect("warm cow snapshot")
            .id,
        entity
    );

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (read_complete, read_complete_rx) = mpsc::channel();
    let read_handle = handle.clone();
    let reader = std::thread::spawn(move || {
        read_complete
            .send(read_handle.snapshot(entity))
            .expect("publish point read");
    });
    let direct = read_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    reader.join().expect("join point reader");
    let snapshot = direct
        .expect("cached point read must not wait for coordinator")
        .expect("direct point read")
        .expect("cow snapshot");
    assert_eq!(snapshot.id, entity);

    handle.status().expect("coordinator resumed");
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_item_stack_cas_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    item.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let entity = handle.spawn(item).expect("item");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("item snapshot");

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(
                mutation_handle
                    .set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2))),
            )
            .expect("publish direct item CAS");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join item mutation");
    assert!(
        direct
            .expect("cached item CAS must not wait for coordinator")
            .expect("direct item CAS")
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("updated item read")
            .expect("updated item snapshot")
            .item_stack,
        Some(crate::EntityItemStack::new(7, 2))
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_single_animal_cas_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut cow = cow(Vec3::new(0.5, 64.0, 0.5));
    cow.animal = Some(AnimalBreedingState::adult());
    let entity = handle.spawn(cow).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    let mut animal = expected.animal.expect("animal state");
    animal.love_ticks = 600;

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.set_animal_states_if_current([(expected, animal)]))
            .expect("publish direct animal CAS");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join animal mutation");
    assert!(
        direct
            .expect("cached animal CAS must not wait for coordinator")
            .expect("direct animal CAS")
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("updated animal read")
            .expect("updated animal snapshot")
            .animal
            .expect("updated animal state")
            .love_ticks,
        600
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_same_lane_animal_batch_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
    first.animal = Some(AnimalBreedingState::adult());
    let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
    second.animal = Some(AnimalBreedingState::adult());
    let first = handle.spawn(first).expect("first cow");
    let second = handle.spawn(second).expect("second cow");
    let selected = HashSet::from([first, second]);
    let mut expected = handle
        .snapshots_for_ids(&selected)
        .expect("warm same-lane routes");
    expected.sort_unstable_by_key(|snapshot| snapshot.id);
    let states = expected
        .into_iter()
        .map(|snapshot| {
            let mut animal = snapshot.animal.expect("animal state");
            animal.age_ticks = -20;
            (snapshot, animal)
        })
        .collect::<Vec<_>>();

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.set_animal_states_if_current(states))
            .expect("publish direct animal batch");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join animal batch");
    assert!(
        direct
            .expect("cached same-lane animal batch must not wait for coordinator")
            .expect("direct animal batch")
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_standalone_kinematics_bypass_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    let next = movement(entity, Vec3::new(1.0, 64.0, 0.5));

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.apply_kinematics_if_current([(expected, next)]))
            .expect("publish direct kinematics CAS");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join kinematics mutation");
    assert!(
        direct
            .expect("cached standalone kinematics must not wait for coordinator")
            .expect("direct kinematics CAS")
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("updated kinematics read")
            .expect("updated cow snapshot")
            .position,
        next.position
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_multi_entity_kinematics_bypass_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
        ])
        .expect("same-region cows");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let expected = handle
        .snapshots_for_ids(&selected)
        .expect("warm selected routes");
    let states = expected.into_iter().map(|snapshot| {
        let target = movement(
            snapshot.id,
            Vec3::new(
                snapshot.position.x + 0.25,
                snapshot.position.y,
                snapshot.position.z,
            ),
        );
        (snapshot, target)
    });

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.apply_kinematics_if_current(states))
            .expect("publish direct kinematics batch");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join kinematics batch");
    assert!(
        direct
            .expect("cached kinematics batch must not wait for coordinator")
            .expect("direct kinematics batch")
    );

    drop(handle);
    let store = runtime.shutdown().expect("runtime shutdown");
    assert_eq!(
        store.stores[&RegionKey::new(0, 0)].physics_apply_stage_runs_for_test(),
        1
    );
}

#[test]
fn multi_entity_kinematics_run_physics_once_per_region() {
    const ENTITY_COUNT: usize = 76;

    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch((0..ENTITY_COUNT).map(|index| {
            cow(Vec3::new(
                0.5 + (index % 16) as f64,
                64.0,
                0.5 + (index / 16) as f64,
            ))
        }))
        .expect("same-region cows");
    let active = ids.iter().copied().collect::<HashSet<_>>();
    let expected = handle
        .snapshots_for_ids(&active)
        .expect("warm selected routes");
    let states = expected.into_iter().map(|snapshot| {
        let target = movement(
            snapshot.id,
            Vec3::new(
                snapshot.position.x + 0.01,
                snapshot.position.y,
                snapshot.position.z,
            ),
        );
        (snapshot, target)
    });

    assert!(
        handle
            .apply_kinematics_if_current(states)
            .expect("batched kinematics")
    );

    drop(handle);
    let store = runtime.shutdown().expect("runtime shutdown");
    let region = store
        .stores
        .get(&RegionKey::new(0, 0))
        .expect("spawn region");
    assert_eq!(region.physics_apply_stage_runs_for_test(), 1);
}

#[test]
fn multi_region_kinematics_run_physics_once_in_each_region() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
            cow(Vec3::new(129.5, 64.0, 0.5)),
        ])
        .expect("two-region cows");
    let active = ids.iter().copied().collect::<HashSet<_>>();
    let states = handle
        .snapshots_for_ids(&active)
        .expect("warm selected routes")
        .into_iter()
        .map(|snapshot| {
            let target = movement(
                snapshot.id,
                Vec3::new(
                    snapshot.position.x + 0.01,
                    snapshot.position.y,
                    snapshot.position.z,
                ),
            );
            (snapshot, target)
        });

    assert!(
        handle
            .apply_kinematics_if_current(states)
            .expect("batched kinematics")
    );

    drop(handle);
    let store = runtime.shutdown().expect("runtime shutdown");
    assert_eq!(
        store.stores[&RegionKey::new(0, 0)].physics_apply_stage_runs_for_test(),
        1
    );
    assert_eq!(
        store.stores[&RegionKey::new(1, 0)].physics_apply_stage_runs_for_test(),
        1
    );
}

#[test]
fn multi_lane_kinematics_run_physics_once_in_each_region() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
            cow(Vec3::new(129.5, 64.0, 0.5)),
        ])
        .expect("two-lane cows");
    let active = ids.iter().copied().collect::<HashSet<_>>();
    let states = handle
        .snapshots_for_ids(&active)
        .expect("warm selected routes")
        .into_iter()
        .map(|snapshot| {
            let target = movement(
                snapshot.id,
                Vec3::new(
                    snapshot.position.x + 0.01,
                    snapshot.position.y,
                    snapshot.position.z,
                ),
            );
            (snapshot, target)
        });

    assert!(
        handle
            .apply_kinematics_if_current(states)
            .expect("multi-lane kinematics")
    );

    drop(handle);
    let store = runtime.shutdown().expect("runtime shutdown");
    assert_ne!(
        store
            .ownership
            .lease(RegionKey::new(0, 0))
            .expect("west lease")
            .lane,
        store
            .ownership
            .lease(RegionKey::new(1, 0))
            .expect("east lease")
            .lane
    );
    assert_eq!(
        store.stores[&RegionKey::new(0, 0)].physics_apply_stage_runs_for_test(),
        1
    );
    assert_eq!(
        store.stores[&RegionKey::new(1, 0)].physics_apply_stage_runs_for_test(),
        1
    );
}

#[test]
fn multi_entity_kinematics_reject_one_stale_snapshot_atomically() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
        ])
        .expect("same-region cows");
    let first = ids[0];
    let second = ids[1];
    let selected = HashSet::from([first, second]);
    let expected = handle
        .snapshots_for_ids(&selected)
        .expect("warm selected routes");
    handle
        .set_goal(
            second,
            GoalState::FollowPosition {
                target: Vec3::new(4.5, 64.0, 0.5),
                speed: 0.25,
            },
        )
        .expect("make second snapshot stale");
    let states = expected.into_iter().map(|snapshot| {
        let target = movement(
            snapshot.id,
            Vec3::new(
                snapshot.position.x + 1.0,
                snapshot.position.y,
                snapshot.position.z,
            ),
        );
        (snapshot, target)
    });

    assert!(
        !handle
            .apply_kinematics_if_current(states)
            .expect("stale batch compare")
    );
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("unchanged authoritative snapshots");
    let first_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == first)
        .expect("first cow");
    let second_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == second)
        .expect("second cow");
    assert_eq!(first_snapshot.position, Vec3::new(0.5, 64.0, 0.5));
    assert_eq!(second_snapshot.position, Vec3::new(1.5, 64.0, 0.5));
    assert_eq!(
        second_snapshot.goal,
        GoalState::FollowPosition {
            target: Vec3::new(4.5, 64.0, 0.5),
            speed: 0.25,
        }
    );

    drop(handle);
    let store = runtime.shutdown().expect("runtime shutdown");
    assert_eq!(
        store.stores[&RegionKey::new(0, 0)].physics_apply_stage_runs_for_test(),
        0
    );
}

#[test]
fn cached_kinematics_batch_rolls_back_safe_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(1.5, 64.0, 0.5)),
        ])
        .expect("same-region cows");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let expected = handle
        .snapshots_for_ids(&selected)
        .expect("warm selected routes");
    let states = expected.iter().cloned().map(|snapshot| {
        let target = movement(
            snapshot.id,
            Vec3::new(
                snapshot.position.x + 1.0,
                snapshot.position.y,
                snapshot.position.z,
            ),
        );
        (snapshot, target)
    });
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.apply_kinematics_if_current(states),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("rolled back batch"),
        expected
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_standalone_kinematics_roll_back_safe_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.apply_kinematics_if_current([(
            expected.clone(),
            movement(entity, Vec3::new(1.0, 64.0, 0.5)),
        )]),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("rolled back read")
            .expect("rolled back cow"),
        expected
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_standalone_kinematics_can_defer_journal() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    let commit_count = journal.lock().expect("journal state").commits.len();
    let next = movement(entity, Vec3::new(1.0, 64.0, 0.5));

    assert_eq!(
        handle
            .apply_kinematics_if_current_deferred_journal_committed([(expected.clone(), next)])
            .expect("checkpoint-only kinematics CAS"),
        vec![next]
    );
    let current = handle
        .snapshot(entity)
        .expect("current read")
        .expect("current cow");
    assert_eq!(
        handle
            .apply_kinematics_if_current_deferred_journal_committed([(
                current,
                movement(entity, Vec3::new(1.5, 64.0, 0.5)),
            )])
            .expect("current kinematics CAS")
            .len(),
        1
    );
    assert!(
        handle
            .apply_kinematics_if_current_deferred_journal_committed([(
                expected,
                movement(entity, Vec3::new(2.0, 64.0, 0.5)),
            )])
            .expect("stale kinematics CAS")
            .is_empty()
    );
    assert_eq!(
        journal.lock().expect("journal state").commits.len(),
        commit_count
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("updated read")
            .expect("updated cow")
            .position,
        Vec3::new(1.5, 64.0, 0.5)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn parallel_deferred_kinematics_versioned_fast_path_returns_current_fence() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("two-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("versioned fast-path entities");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm versioned fast-path routes");
    let states = snapshots
        .iter()
        .map(|snapshot| {
            let next = Vec3::new(
                snapshot.position.x + 1.0,
                snapshot.position.y,
                snapshot.position.z,
            );
            (snapshot.clone(), movement(snapshot.id, next))
        })
        .collect::<Vec<_>>();

    let committed = handle
        .try_apply_kinematics_if_current_parallel_deferred_journal_versioned(&states, 2)
        .expect("versioned fast-path commit")
        .expect("cached multi-lane fast path");
    assert_eq!(committed.snapshots().len(), states.len());
    for snapshot in committed.snapshots() {
        let target = states
            .iter()
            .find(|(_, state)| state.id == snapshot.id)
            .map(|(_, state)| state.position)
            .expect("committed target");
        assert_eq!(snapshot.position, target);
    }
    assert!(
        handle
            .versioned_snapshots_are_current(&committed)
            .expect("current version fence"),
        "freshly committed fast-path snapshots must be current"
    );

    let changed = committed.snapshots()[0].id;
    handle
        .set_velocities([(changed, Vec3::new(0.25, 0.0, 0.0))])
        .expect("mutate one fenced lane");
    assert!(
        !handle
            .versioned_snapshots_are_current(&committed)
            .expect("stale version fence"),
        "a later mutation on a returned entity must stale the fence"
    );
    drop(handle);
    runtime.shutdown().expect("versioned fast-path shutdown");
}

#[test]
fn parallel_deferred_kinematics_commits_multi_lane_batch() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("two-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("multi-lane kinematics entities");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm cross-lane kinematics routes");
    let states = snapshots
        .iter()
        .map(|snapshot| {
            let next = Vec3::new(
                snapshot.position.x + 1.0,
                snapshot.position.y,
                snapshot.position.z,
            );
            (snapshot.clone(), movement(snapshot.id, next))
        })
        .collect::<Vec<_>>();
    let commit_count = journal.lock().expect("journal state").commits.len();

    let committed = handle
        .apply_kinematics_if_current_parallel_deferred_journal_committed(states.clone(), 2)
        .expect("parallel multi-lane kinematics batch");

    assert_eq!(
        committed,
        states.iter().map(|(_, state)| *state).collect::<Vec<_>>()
    );
    assert_eq!(
        journal.lock().expect("journal state").commits.len(),
        commit_count,
        "deferred journal commits must stay unwritten"
    );
    drop(handle);
    runtime.shutdown().expect("multi-lane parallel shutdown");
}

#[test]
fn parallel_deferred_kinematics_stale_lane_rolls_back_fresh_lane() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("two-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("stale lane kinematics entities");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm cross-lane kinematics routes");
    let stale = snapshots[0].clone();
    let fresh = snapshots[1].clone();
    let stale_next = Vec3::new(stale.position.x + 2.0, stale.position.y, stale.position.z);
    let moved = handle
        .apply_kinematics_if_current_deferred_journal_committed([(
            stale.clone(),
            movement(stale.id, stale_next),
        )])
        .expect("serial stale-making move");
    assert_eq!(moved.len(), 1);

    let stale_staler = Vec3::new(stale.position.x + 3.0, stale.position.y, stale.position.z);
    let fresh_next = Vec3::new(fresh.position.x + 1.0, fresh.position.y, fresh.position.z);
    let committed = handle
        .apply_kinematics_if_current_parallel_deferred_journal_committed(
            [
                (stale.clone(), movement(stale.id, stale_staler)),
                (fresh.clone(), movement(fresh.id, fresh_next)),
            ],
            2,
        )
        .expect("parallel batch with one stale lane");

    assert!(
        committed.is_empty(),
        "one stale lane must keep the whole batch from committing"
    );
    assert_eq!(
        handle
            .snapshot(stale.id)
            .expect("stale lane read")
            .expect("stale cow")
            .position,
        stale_next,
        "stale entity must stay at its pre-call position"
    );
    assert_eq!(
        handle
            .snapshot(fresh.id)
            .expect("fresh lane read")
            .expect("fresh cow")
            .position,
        fresh.position,
        "fresh entity on a batch with a stale lane must not move"
    );
    drop(handle);
    runtime.shutdown().expect("stale lane shutdown");
}

#[test]
fn parallel_deferred_kinematics_more_lanes_than_workers_commits_all() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        3,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("three-lane owner runtime");
    let handle = runtime.handle();
    let ids = handle
        .spawn_batch([
            cow(Vec3::new(0.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
            cow(Vec3::new(256.5, 64.0, 0.5)),
        ])
        .expect("three-lane kinematics entities");
    let selected = ids.iter().copied().collect::<HashSet<_>>();
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm three-lane kinematics routes");
    let states = snapshots
        .iter()
        .map(|snapshot| {
            let next = Vec3::new(
                snapshot.position.x + 1.0,
                snapshot.position.y,
                snapshot.position.z,
            );
            (snapshot.clone(), movement(snapshot.id, next))
        })
        .collect::<Vec<_>>();
    let commit_count = journal.lock().expect("journal state").commits.len();

    let committed = handle
        .apply_kinematics_if_current_parallel_deferred_journal_committed(states.clone(), 2)
        .expect("three-lane batch with two workers");

    let mut expected = states.iter().map(|(_, state)| *state).collect::<Vec<_>>();
    expected.sort_unstable_by_key(|kinematics| kinematics.id);
    assert_eq!(committed, expected, "every lane must commit, sorted by id");
    assert_eq!(
        journal.lock().expect("journal state").commits.len(),
        commit_count,
        "deferred journal commits must stay unwritten"
    );
    for (_, state) in &states {
        assert_eq!(
            handle
                .snapshot(state.id)
                .expect("three-lane read")
                .expect("three-lane cow")
                .position,
            state.position
        );
    }
    drop(handle);
    runtime.shutdown().expect("three-lane parallel shutdown");
}

#[test]
fn parallel_deferred_kinematics_single_worker_matches_serial_path() {
    let seed = || {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let ids = handle
            .spawn_batch([
                cow(Vec3::new(0.5, 64.0, 0.5)),
                cow(Vec3::new(128.5, 64.0, 0.5)),
            ])
            .expect("single worker kinematics entities");
        let selected = ids.iter().copied().collect::<HashSet<_>>();
        let snapshots = handle
            .snapshots_for_ids(&selected)
            .expect("warm cross-lane kinematics routes");
        let states = snapshots
            .iter()
            .map(|snapshot| {
                let next = Vec3::new(
                    snapshot.position.x + 1.0,
                    snapshot.position.y,
                    snapshot.position.z,
                );
                (snapshot.clone(), movement(snapshot.id, next))
            })
            .collect::<Vec<_>>();
        (runtime, handle, states)
    };
    let (serial_runtime, serial_handle, serial_states) = seed();
    let (parallel_runtime, parallel_handle, parallel_states) = seed();

    let serial = serial_handle
        .apply_kinematics_if_current_deferred_journal_committed(serial_states)
        .expect("serial kinematics batch");
    let parallel = parallel_handle
        .apply_kinematics_if_current_parallel_deferred_journal_committed(parallel_states, 1)
        .expect("single-worker kinematics batch");

    assert_eq!(parallel, serial);
    for (serial_kinematics, parallel_kinematics) in serial.iter().zip(&parallel) {
        assert_eq!(serial_kinematics.id, parallel_kinematics.id);
        let serial_snapshot = serial_handle
            .snapshot(serial_kinematics.id)
            .expect("serial read")
            .expect("serial cow");
        let parallel_snapshot = parallel_handle
            .snapshot(parallel_kinematics.id)
            .expect("parallel read")
            .expect("parallel cow");
        assert_eq!(parallel_snapshot, serial_snapshot);
    }
    drop(serial_handle);
    serial_runtime.shutdown().expect("serial shutdown");
    drop(parallel_handle);
    parallel_runtime.shutdown().expect("parallel shutdown");
}

#[test]
fn concurrent_cached_standalone_kinematics_apply_one_exact_snapshot() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    let barrier = Arc::new(std::sync::Barrier::new(3));
    let first_position = Vec3::new(1.0, 64.0, 0.5);
    let second_position = Vec3::new(1.5, 64.0, 0.5);

    let (first, second) = std::thread::scope(|scope| {
        let first_handle = handle.clone();
        let first_expected = expected.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = scope.spawn(move || {
            first_barrier.wait();
            first_handle
                .apply_kinematics_if_current([(first_expected, movement(entity, first_position))])
        });
        let second_handle = handle.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            second_handle
                .apply_kinematics_if_current([(expected, movement(entity, second_position))])
        });
        barrier.wait();
        (
            first.join().expect("first kinematics worker"),
            second.join().expect("second kinematics worker"),
        )
    });
    let applied = [
        first.expect("first kinematics"),
        second.expect("second kinematics"),
    ]
    .into_iter()
    .filter(|applied| *applied)
    .count();
    assert_eq!(applied, 1);
    assert!(
        [first_position, second_position].contains(
            &handle
                .snapshot(entity)
                .expect("final read")
                .expect("final cow")
                .position
        )
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_same_lane_non_reference_goals_bypass_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let first = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("first cow");
    let second = handle
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("second cow");
    let selected = HashSet::from([first, second]);
    assert_eq!(
        handle.snapshots().expect("warm full-snapshot routes").len(),
        2
    );
    let goal = GoalState::FollowPosition {
        target: Vec3::new(8.5, 64.0, 8.5),
        speed: 0.25,
    };

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let expected_goal = goal.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.set_goals([(first, goal.clone()), (second, goal)]))
            .expect("publish direct goal batch");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join goal batch");
    assert_eq!(
        direct.expect("cached same-lane goals must not wait for coordinator"),
        Ok(2)
    );
    assert!(
        handle
            .snapshots_for_ids(&selected)
            .expect("updated goals")
            .iter()
            .all(|snapshot| snapshot.goal == expected_goal)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_follow_target_goal_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let target = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("target cow");
    let follower = handle
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("follower cow");
    let selected = HashSet::from([target, follower]);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("warm follower and target routes")
            .len(),
        2
    );
    let goal = GoalState::FollowTarget {
        target,
        speed: 0.25,
    };

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let expected_goal = goal.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.set_goals([(follower, goal)]))
            .expect("publish direct follow-target goal");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join follow-target goal mutation");
    assert_eq!(
        direct.expect("cached follow-target goal must not wait for coordinator"),
        Ok(1)
    );
    assert_eq!(
        handle
            .snapshot(follower)
            .expect("updated follower read")
            .expect("follower snapshot")
            .goal,
        expected_goal
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_follow_target_goal_ignores_unrelated_lane_writer() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let target = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("target cow");
    let follower = handle
        .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
        .expect("follower cow");
    let unrelated = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("unrelated cow");
    let selected = HashSet::from([target, follower, unrelated]);
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm all routes");
    let unrelated_snapshot = snapshots
        .into_iter()
        .find(|snapshot| snapshot.id == unrelated)
        .expect("unrelated snapshot");
    {
        let routes = handle
            .selected_read_routes
            .read()
            .expect("regional test lock poisoned");
        assert_eq!(routes[&target].lease.lane, routes[&follower].lease.lane);
        assert_ne!(routes[&target].lease.lane, routes[&unrelated].lease.lane);
    }

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (read_entered, read_entered_rx) = mpsc::channel();
    let (read_release, read_release_rx) = mpsc::channel();
    handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
    let mutation_handle = handle.clone();
    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.set_goals([(
                follower,
                GoalState::FollowTarget {
                    target,
                    speed: 0.25,
                },
            )]))
            .expect("publish referenced goal mutation");
    });
    read_entered_rx
        .recv()
        .expect("referenced read dispatched to owner lanes");

    assert!(
        handle
            .apply_kinematics_if_current([(
                unrelated_snapshot,
                movement(unrelated, Vec3::new(129.0, 64.0, 0.5)),
            )])
            .expect("unrelated lane mutation")
    );
    read_release.send(()).expect("release referenced read");
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join referenced mutation");

    assert_eq!(
        direct.expect("unrelated lane writer must not force actor fallback"),
        Ok(1)
    );
    assert_eq!(
        handle
            .snapshot(follower)
            .expect("follower read")
            .expect("follower snapshot")
            .goal,
        GoalState::FollowTarget {
            target,
            speed: 0.25,
        }
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_follow_target_goal_falls_back_after_target_migration() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let target = handle
        .spawn(cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("target cow");
    let follower = handle
        .spawn(cow(Vec3::new(126.5, 64.0, 0.5)))
        .expect("follower cow");
    let selected = HashSet::from([target, follower]);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("warm follower and target routes")
            .len(),
        2
    );

    let (read_entered, read_entered_rx) = mpsc::channel();
    let (read_release, read_release_rx) = mpsc::channel();
    handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
    let (fallback_entered, fallback_entered_rx) = mpsc::channel();
    handle.notify_referenced_goal_fallback_for_test(fallback_entered);
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_handle.set_goals([(
            follower,
            GoalState::FollowTarget {
                target,
                speed: 0.25,
            },
        )])
    });
    if let Err(error) = read_entered_rx.recv_timeout(Duration::from_secs(1)) {
        *handle
            .selected_read_probe
            .lock()
            .expect("regional test lock poisoned") = None;
        let result = mutation.join().expect("join missing direct read");
        panic!("follow-target selected read was not dispatched: {error}; result: {result:?}");
    }

    assert!(
        handle
            .set_position(target, Vec3::new(128.5, 64.0, 0.5))
            .expect("migrate target")
    );

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");
    read_release.send(()).expect("release selected read");
    fallback_entered_rx
        .recv()
        .expect("version fence rejected direct follow-target goal");
    assert_eq!(
        handle
            .snapshot(follower)
            .expect("follower read before fallback")
            .expect("follower snapshot")
            .goal,
        GoalState::Idle
    );
    coordinator_release.send(()).expect("release coordinator");
    assert_eq!(mutation.join().expect("join follow-target fallback"), Ok(1));
    assert_eq!(
        handle
            .snapshot(follower)
            .expect("follower read after fallback")
            .expect("follower snapshot")
            .goal,
        GoalState::FollowTarget {
            target,
            speed: 0.25,
        }
    );
    assert_eq!(
        handle
            .snapshot(target)
            .expect("migrated target read")
            .expect("target snapshot")
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_same_lane_animal_batch_records_one_durable_decision() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
    first.animal = Some(AnimalBreedingState::adult());
    let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
    second.animal = Some(AnimalBreedingState::adult());
    let ids = [
        handle.spawn(first).expect("first cow"),
        handle.spawn(second).expect("second cow"),
    ];
    let mut expected = handle
        .snapshots_for_ids(&HashSet::from(ids))
        .expect("warm same-lane routes");
    expected.sort_unstable_by_key(|snapshot| snapshot.id);
    let states = expected
        .into_iter()
        .map(|snapshot| {
            let mut animal = snapshot.animal.expect("animal state");
            animal.age_ticks = -20;
            (snapshot, animal)
        })
        .collect::<Vec<_>>();

    assert!(
        handle
            .set_animal_states_if_current(states)
            .expect("direct animal batch")
    );

    let state = journal.lock().expect("journal state");
    let decision = state.commits.last().expect("batch durable decision");
    assert_eq!(decision.upserts().len(), 2);
    assert!(decision.upserts().iter().all(|snapshot| {
        snapshot
            .animal
            .is_some_and(|animal| animal.age_ticks == -20)
    }));
    drop(state);

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_same_lane_animal_batch_can_defer_journal() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut animal = cow(Vec3::new(0.5, 64.0, 0.5));
    animal.animal = Some(AnimalBreedingState::baby());
    let id = handle.spawn(animal).expect("baby cow");
    let expected = handle
        .snapshot(id)
        .expect("warm point route")
        .expect("baby cow snapshot");
    let commit_count = journal.lock().expect("journal state").commits.len();
    let mut next = expected.animal.expect("animal state");
    next.age_ticks += 1;

    assert!(
        handle
            .set_animal_states_if_current_deferred_journal([(expected, next)])
            .expect("checkpoint-only animal CAS")
    );

    assert_eq!(
        journal.lock().expect("journal state").commits.len(),
        commit_count
    );
    assert_eq!(
        handle
            .snapshot(id)
            .expect("updated point read")
            .expect("baby cow remains")
            .animal,
        Some(next)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_same_lane_animal_batch_rolls_back_safe_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
    first.animal = Some(AnimalBreedingState::adult());
    let mut second = cow(Vec3::new(1.5, 64.0, 0.5));
    second.animal = Some(AnimalBreedingState::adult());
    let ids = [
        handle.spawn(first).expect("first cow"),
        handle.spawn(second).expect("second cow"),
    ];
    let selected = HashSet::from(ids);
    let states = handle
        .snapshots_for_ids(&selected)
        .expect("warm same-lane routes")
        .into_iter()
        .map(|snapshot| {
            let mut animal = snapshot.animal.expect("animal state");
            animal.age_ticks = -20;
            (snapshot, animal)
        })
        .collect::<Vec<_>>();
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.set_animal_states_if_current(states),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert!(
        handle
            .snapshots_for_ids(&selected)
            .expect("rolled back snapshots")
            .iter()
            .all(|snapshot| snapshot.animal.is_some_and(|animal| animal.age_ticks == 0))
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_goal_rolls_back_exact_snapshot_on_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    assert!(
        handle
            .set_goal(
                entity,
                GoalState::FollowPosition {
                    target: Vec3::new(4.5, 64.0, 0.5),
                    speed: 0.25,
                },
            )
            .expect("set pathing goal")
    );
    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(HashSet::from([entity])))
        .expect("prepare retained path state");
    handle
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("commit retained path state");
    let before = handle
        .snapshot(entity)
        .expect("snapshot read")
        .expect("cow snapshot");
    assert_ne!(before.retained, crate::EntityRetainedState::default());
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.set_goal(entity, GoalState::Idle),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("rolled back read")
            .expect("rolled back cow"),
        before,
        "goal rollback must preserve the exact goal and retained path state"
    );
    journal.lock().expect("journal state").fail_record = false;
    assert_eq!(
        handle
            .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("spawn after rollback"),
        EntityId(entity.0 + 1),
        "goal rollback must preserve the entity ID cursor"
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_goal_batch_rolls_back_exact_snapshots_on_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let ids = [
        handle
            .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
            .expect("first cow"),
        handle
            .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("second cow"),
    ];
    assert_eq!(
        handle
            .set_goals(ids.map(|entity| {
                (
                    entity,
                    GoalState::FollowPosition {
                        target: Vec3::new(6.5, 64.0, 0.5),
                        speed: 0.25,
                    },
                )
            }))
            .expect("set pathing goals"),
        2
    );
    let selected = HashSet::from(ids);
    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(selected.clone()))
        .expect("prepare retained path states");
    handle
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("commit retained path states");
    let mut before = handle.snapshots_for_ids(&selected).expect("snapshot reads");
    before.sort_by_key(|snapshot| snapshot.id);
    assert!(
        before
            .iter()
            .all(|snapshot| { snapshot.retained != crate::EntityRetainedState::default() })
    );
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.set_goals(ids.map(|entity| (entity, GoalState::Idle))),
        Err(super::RegionOwnerLaneError::Journal)
    );
    let mut after = handle
        .snapshots_for_ids(&selected)
        .expect("rolled back reads");
    after.sort_by_key(|snapshot| snapshot.id);
    assert_eq!(
        after, before,
        "batch goal rollback must preserve exact goals and retained path states"
    );
    journal.lock().expect("journal state").fail_record = false;
    assert_eq!(
        handle
            .spawn(cow(Vec3::new(2.5, 64.0, 0.5)))
            .expect("spawn after rollback"),
        EntityId(ids[1].0 + 1),
        "batch goal rollback must preserve the entity ID cursor"
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_damage_cas_bypasses_the_coordinator_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (mutation_complete, mutation_complete_rx) = mpsc::channel();
    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_complete
            .send(mutation_handle.damage_if_current(expected, damage_request(1.0)))
            .expect("publish direct damage CAS");
    });
    let direct = mutation_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    mutation.join().expect("join damage mutation");
    let damage = direct
        .expect("cached damage CAS must not wait for coordinator")
        .expect("direct damage CAS")
        .expect("damage applied");
    assert_eq!(damage.snapshot.id, entity);
    assert!(damage.snapshot.health < 20.0);
    assert!(!damage.killed);

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_damage_cas_rolls_back_safe_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    assert!(
        handle
            .set_goal(
                entity,
                GoalState::Wander {
                    speed: 0.8,
                    period_ticks: 80,
                },
            )
            .expect("set wander goal")
    );
    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids(1, Arc::new(HashSet::from([entity])))
        .expect("prepare retained path state");
    handle
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("commit retained path state");
    let before_timers = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    assert_ne!(
        before_timers.retained,
        crate::EntityRetainedState::default()
    );
    let mut expected = before_timers.clone();
    expected.retained.last_damage_tick = Some(3);
    expected.retained.death_remove_tick = Some(31);
    expected.retained.sheep_grazing_ticks = Some(5);
    let effect_id = EffectId::new(99);
    expected.retained.active_effects = Some(EntityActiveEffectsState {
        effects: ActiveEffectsSnapshot {
            chains: vec![ActiveEffectChainSnapshot {
                current: EffectInstance::new(
                    effect_id,
                    EffectKind::CallerOwned,
                    80,
                    1,
                    EffectFlags::default(),
                ),
                hidden: vec![EffectInstance::new(
                    effect_id,
                    EffectKind::CallerOwned,
                    160,
                    0,
                    EffectFlags::default(),
                )],
            }],
        },
        action_order: vec![effect_id],
    });
    assert!(
        handle
            .replace_snapshot_if_current(before_timers, expected.clone())
            .expect("commit retained decision state")
    );
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.damage_if_current(expected.clone(), damage_request(1.0)),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("rolled back read")
            .expect("rolled back cow"),
        expected,
        "journal rollback must restore every retained ECS component"
    );
    journal.lock().expect("journal state").fail_record = false;
    assert_eq!(
        handle
            .spawn(cow(Vec3::new(1.5, 64.0, 0.5)))
            .expect("spawn after rollback"),
        EntityId(entity.0 + 1),
        "journal rollback must preserve the entity ID cursor"
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_damage_cas_reports_lethal_post_state() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");

    let damage = handle
        .damage_if_current(expected, damage_request(100.0))
        .expect("direct lethal damage")
        .expect("damage applied");
    assert!(damage.killed);
    assert_eq!(
        damage.snapshot.lifecycle,
        crate::EntityLifecycle::Despawning
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn concurrent_cached_damage_cas_applies_one_exact_snapshot() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("cow snapshot");
    let initial_health = expected.health;
    let barrier = Arc::new(std::sync::Barrier::new(3));

    let (first, second) = std::thread::scope(|scope| {
        let first_handle = handle.clone();
        let first_expected = expected.clone();
        let first_barrier = Arc::clone(&barrier);
        let first = scope.spawn(move || {
            first_barrier.wait();
            first_handle.damage_if_current(first_expected, damage_request(1.0))
        });
        let second_handle = handle.clone();
        let second_barrier = Arc::clone(&barrier);
        let second = scope.spawn(move || {
            second_barrier.wait();
            second_handle.damage_if_current(expected, damage_request(1.0))
        });
        barrier.wait();
        (
            first.join().expect("first damage worker"),
            second.join().expect("second damage worker"),
        )
    });
    let applied = [first.expect("first damage"), second.expect("second damage")]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    assert_eq!(applied.len(), 1);
    assert!(applied[0].snapshot.health < initial_health);
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("final read")
            .expect("final cow")
            .health,
        applied[0].snapshot.health
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_item_stack_cas_records_post_state_and_save_phase() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    item.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let entity = handle.spawn(item).expect("item");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("item snapshot");

    assert!(
        handle
            .set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2)),)
            .expect("direct item CAS")
    );

    let state = journal.lock().expect("journal state");
    let decision = state.commits.last().expect("direct durable decision");
    assert_eq!(decision.upserts().len(), 1);
    assert_eq!(decision.upserts()[0].id, entity);
    assert_eq!(
        decision.upserts()[0].item_stack,
        Some(crate::EntityItemStack::new(7, 2))
    );
    let phase = decision.phase();
    drop(state);
    assert!(
        handle
            .save_barrier()
            .expect("save barrier")
            .journal_phases()
            .contains(&phase)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn item_merge_cas_updates_survivor_and_removes_consumed_atomically() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut survivor = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    survivor.item_stack = Some(crate::EntityItemStack::new(7, 3));
    survivor.retained.spawn_tick = 4;
    let survivor_id = handle.spawn(survivor).expect("survivor item");
    let mut consumed = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.75, 64.0, 0.5));
    consumed.item_stack = Some(crate::EntityItemStack::new(7, 2));
    consumed.retained.spawn_tick = 9;
    let consumed_id = handle.spawn(consumed).expect("consumed item");
    let survivor_expected = handle
        .snapshot(survivor_id)
        .expect("survivor read")
        .expect("survivor snapshot");
    let consumed_expected = handle
        .snapshot(consumed_id)
        .expect("consumed read")
        .expect("consumed snapshot");
    let mut survivor_next = survivor_expected.clone();
    survivor_next.item_stack = Some(crate::EntityItemStack::new(7, 5));
    survivor_next.retained.spawn_tick = 9;

    assert!(
        handle
            .merge_item_snapshots_if_current(
                survivor_expected,
                survivor_next.clone(),
                consumed_expected,
            )
            .expect("merge CAS")
    );
    assert_eq!(
        handle.snapshot(survivor_id).expect("merged survivor read"),
        Some(survivor_next)
    );
    assert!(
        handle
            .snapshot(consumed_id)
            .expect("consumed removal read")
            .is_none()
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn stale_item_merge_cas_changes_neither_entity() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut survivor = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    survivor.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let survivor_id = handle.spawn(survivor).expect("survivor item");
    let mut consumed = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.75, 64.0, 0.5));
    consumed.item_stack = Some(crate::EntityItemStack::new(7, 2));
    let consumed_id = handle.spawn(consumed).expect("consumed item");
    let survivor_expected = handle
        .snapshot(survivor_id)
        .expect("survivor read")
        .expect("survivor snapshot");
    let stale_consumed = handle
        .snapshot(consumed_id)
        .expect("consumed read")
        .expect("consumed snapshot");
    assert!(
        handle
            .set_item_stack_if_current(
                stale_consumed.clone(),
                Some(crate::EntityItemStack::new(7, 1)),
            )
            .expect("change consumed before stale merge")
    );
    let current_consumed = handle
        .snapshot(consumed_id)
        .expect("current consumed read")
        .expect("current consumed snapshot");
    let mut survivor_next = survivor_expected.clone();
    survivor_next.item_stack = Some(crate::EntityItemStack::new(7, 5));

    assert!(
        !handle
            .merge_item_snapshots_if_current(
                survivor_expected.clone(),
                survivor_next,
                stale_consumed,
            )
            .expect("stale merge CAS")
    );
    assert_eq!(
        handle
            .snapshot(survivor_id)
            .expect("survivor after stale merge"),
        Some(survivor_expected)
    );
    assert_eq!(
        handle
            .snapshot(consumed_id)
            .expect("consumed after stale merge"),
        Some(current_consumed)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cross_region_item_merge_rolls_back_both_entities_on_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut survivor = SpawnEntity::new(1, "minecraft:item", Vec3::new(127.9, 64.0, 0.5));
    survivor.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let survivor_id = handle.spawn(survivor).expect("survivor item");
    let mut consumed = SpawnEntity::new(1, "minecraft:item", Vec3::new(128.1, 64.0, 0.5));
    consumed.item_stack = Some(crate::EntityItemStack::new(7, 2));
    let consumed_id = handle.spawn(consumed).expect("consumed item");
    let survivor_expected = handle
        .snapshot(survivor_id)
        .expect("survivor read")
        .expect("survivor snapshot");
    let consumed_expected = handle
        .snapshot(consumed_id)
        .expect("consumed read")
        .expect("consumed snapshot");
    let mut survivor_next = survivor_expected.clone();
    survivor_next.item_stack = Some(crate::EntityItemStack::new(7, 5));
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.merge_item_snapshots_if_current(
            survivor_expected.clone(),
            survivor_next,
            consumed_expected.clone(),
        ),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshot(survivor_id)
            .expect("survivor rollback read"),
        Some(survivor_expected)
    );
    assert_eq!(
        handle
            .snapshot(consumed_id)
            .expect("consumed rollback read"),
        Some(consumed_expected)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_item_stack_cas_rolls_back_safe_journal_failure() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    item.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let entity = handle.spawn(item).expect("item");
    let expected = handle
        .snapshot(entity)
        .expect("warm point route")
        .expect("item snapshot");
    journal.lock().expect("journal state").fail_record = true;

    assert_eq!(
        handle.set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2)),),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle
            .snapshot(entity)
            .expect("item read")
            .expect("item snapshot")
            .item_stack,
        Some(crate::EntityItemStack::new(7, 3))
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_unknown_journal_outcome_blocks_point_and_batch_publication() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let mut item = SpawnEntity::new(1, "minecraft:item", Vec3::new(0.5, 64.0, 0.5));
    item.item_stack = Some(crate::EntityItemStack::new(7, 3));
    let entity = handle.spawn(item).expect("item");
    let expected = handle.snapshot(entity).unwrap().unwrap();
    journal.lock().expect("journal state").fail_record_unknown = true;

    assert_eq!(
        handle.set_item_stack_if_current(expected, Some(crate::EntityItemStack::new(7, 2)),),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(
        handle.snapshot(entity),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(
        handle.snapshots(),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    assert_eq!(
        handle.status(),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );

    drop(handle);
    let shutdown = runtime
        .shutdown()
        .expect_err("unknown outcome remains fail-stopped");
    let super::RegionalOwnerRuntimeShutdownError::Owner(shutdown) = shutdown else {
        panic!("unknown outcome must surface through controlled owner shutdown");
    };
    assert_eq!(shutdown.error, super::RegionOwnerLaneError::OutcomeUnknown);
}

#[test]
fn coordinator_worker_panic_is_typed_and_future_requests_fail_closed() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    handle
        .panic_coordinator_for_test()
        .expect("panic command enters bounded owner queue");

    assert_eq!(
        handle.status(),
        Err(super::RegionOwnerLaneError::WorkerPanicked)
    );
    assert_eq!(
        handle.fatal_error(),
        Some(super::RegionOwnerLaneError::WorkerPanicked)
    );
    assert!(matches!(
        runtime.shutdown(),
        Err(super::RegionalOwnerRuntimeShutdownError::WorkerPanicked)
    ));
}

#[test]
fn direct_selected_read_admission_reserves_lane_queue_capacity() {
    let active = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let mut permits = (0..super::DIRECT_SELECTED_READ_LIMIT)
        .map(|_| {
            super::DirectSelectedReadPermit::try_acquire(Arc::clone(&active))
                .expect("admitted direct read")
        })
        .collect::<Vec<_>>();
    assert!(super::DirectSelectedReadPermit::try_acquire(Arc::clone(&active)).is_none());
    permits.pop();
    assert!(super::DirectSelectedReadPermit::try_acquire(active).is_some());
}

#[test]
fn read_commands_select_their_owner_lane_scope() {
    let entity = EntityId(7);
    let (reply, _) = mpsc::channel();
    let point = super::RegionalOwnerCommand::Snapshot { entity, reply };
    assert!(matches!(
        point.lane_scope(),
        Some(super::RegionalOwnerLaneScope::Entities(entities))
            if entities == vec![entity]
    ));

    let selected_ids = HashSet::from([entity, EntityId(8)]);
    let (reply, _) = mpsc::channel();
    let selected = super::RegionalOwnerCommand::SnapshotsForIds {
        entities: selected_ids.clone(),
        selection: super::SnapshotSelection::All,
        reply,
    };
    assert!(matches!(
        selected.lane_scope(),
        Some(super::RegionalOwnerLaneScope::Entities(entities))
            if entities.iter().copied().collect::<HashSet<_>>() == selected_ids
    ));

    let (reply, _) = mpsc::channel();
    assert!(matches!(
        super::RegionalOwnerCommand::Snapshots { reply }.lane_scope(),
        Some(super::RegionalOwnerLaneScope::All)
    ));
    let (reply, _) = mpsc::channel();
    assert!(matches!(
        super::RegionalOwnerCommand::PrepareGoalTick {
            tick: 1,
            active_ids: Arc::new(HashSet::from([entity])),
            selected: None,
            inputs: super::RegionalGoalTickInputs::default(),
            reply,
        }
        .lane_scope(),
        Some(super::RegionalOwnerLaneScope::TopologyOnly)
    ));
}

#[test]
fn malformed_read_route_retains_exclusive_topology_fence() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let key = coordinator.locations[&entity];
    let lease = coordinator.ownership.lease(key).expect("cow lease");
    coordinator
        .ownership
        .unassign(lease)
        .expect("remove route for malformed-state test");

    let (reply, _) = mpsc::channel();
    let command = super::RegionalOwnerCommand::Snapshot { entity, reply };
    assert!(matches!(
        super::topology_access_for_command(&coordinator, &command),
        super::RegionalOwnerTopologyAccess::Exclusive
    ));

    assert_eq!(
        coordinator
            .ownership
            .assign(key, lease.lane)
            .expect("restore cow route"),
        lease
    );
    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn actor_lane_admission_keeps_unrelated_direct_lane_running() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let selected = HashSet::from([west, east]);
    let snapshots = handle
        .snapshots_for_ids(&selected)
        .expect("warm lane routes");
    let west_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == west)
        .expect("west snapshot")
        .clone();
    let east_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == east)
        .expect("east snapshot")
        .clone();
    {
        let routes = handle
            .selected_read_routes
            .read()
            .expect("regional test lock poisoned");
        assert_ne!(routes[&west].lease.lane, routes[&east].lease.lane);
    }

    let (lane_entered, lane_entered_rx) = mpsc::channel();
    let (lane_release, lane_release_rx) = mpsc::channel();
    handle
        .hold_owner_lane_for_test(west, lane_entered, lane_release_rx)
        .expect("queue west lane hold");
    lane_entered_rx.recv().expect("west lane admission held");

    let (east_complete, east_complete_rx) = mpsc::channel();
    let east_handle = handle.clone();
    let east_worker = std::thread::spawn(move || {
        east_complete
            .send(east_handle.apply_kinematics_if_current([(
                east_snapshot,
                movement(east, Vec3::new(129.0, 64.0, 0.5)),
            )]))
            .expect("publish east mutation");
    });
    let east_result = east_complete_rx.recv_timeout(Duration::from_secs(1));

    let (west_complete, west_complete_rx) = mpsc::channel();
    let (west_contended, west_contended_rx) = mpsc::channel();
    handle.notify_direct_mutation_admission_for_test(west_contended);
    let west_handle = handle.clone();
    let west_worker = std::thread::spawn(move || {
        west_complete
            .send(west_handle.apply_kinematics_if_current([(
                west_snapshot,
                movement(west, Vec3::new(1.0, 64.0, 0.5)),
            )]))
            .expect("publish west mutation");
    });
    assert!(
        west_contended_rx
            .recv()
            .expect("west direct mutation reached its lane admission")
    );
    assert_eq!(west_complete_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    lane_release.send(()).expect("release west lane");
    east_worker.join().expect("join east mutation");
    west_worker.join().expect("join west mutation");
    assert_eq!(
        east_result.expect("east lane must progress while west actor lane is held"),
        Ok(true)
    );
    assert_eq!(
        west_complete_rx.recv().expect("west result after release"),
        Ok(true)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn actor_snapshot_blocks_only_its_owner_lane() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let snapshots = handle
        .snapshots_for_ids(&HashSet::from([west, east]))
        .expect("warm lane routes");
    let west_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == west)
        .expect("west snapshot")
        .clone();
    let east_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == east)
        .expect("east snapshot")
        .clone();
    {
        let mut routes = handle
            .selected_read_routes
            .write()
            .expect("regional test lock poisoned");
        assert_ne!(routes[&west].lease.lane, routes[&east].lease.lane);
        routes.remove(&west);
    }

    let (snapshot_entered, snapshot_entered_rx) = mpsc::channel();
    let (snapshot_release, snapshot_release_rx) = mpsc::channel();
    handle.pause_actor_snapshot_after_read_for_test(snapshot_entered, snapshot_release_rx);
    let snapshot_handle = handle.clone();
    let snapshot_worker = std::thread::spawn(move || snapshot_handle.snapshot(west));
    snapshot_entered_rx
        .recv()
        .expect("actor snapshot acquired west admission");

    assert!(
        handle
            .apply_kinematics_if_current([(
                east_snapshot,
                movement(east, Vec3::new(129.0, 64.0, 0.5)),
            )])
            .expect("east direct mutation")
    );

    let (west_complete, west_complete_rx) = mpsc::channel();
    let (west_contended, west_contended_rx) = mpsc::channel();
    handle.notify_direct_mutation_admission_for_test(west_contended);
    let west_handle = handle.clone();
    let west_worker = std::thread::spawn(move || {
        west_complete
            .send(west_handle.apply_kinematics_if_current([(
                west_snapshot,
                movement(west, Vec3::new(1.0, 64.0, 0.5)),
            )]))
            .expect("publish west mutation");
    });
    assert!(
        west_contended_rx
            .recv()
            .expect("west direct mutation reached its lane admission")
    );
    assert_eq!(west_complete_rx.try_recv(), Err(mpsc::TryRecvError::Empty));

    snapshot_release.send(()).expect("release actor snapshot");
    assert_eq!(
        snapshot_worker
            .join()
            .expect("join actor snapshot")
            .expect("actor snapshot result")
            .expect("west cow")
            .id,
        west
    );
    west_worker.join().expect("join west mutation");
    assert_eq!(
        west_complete_rx.recv().expect("west result after release"),
        Ok(true)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn goal_preparation_does_not_hold_unrelated_lane_admission() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let snapshots = handle
        .snapshots_for_ids(&HashSet::from([west, east]))
        .expect("warm lane routes");
    let east_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == east)
        .expect("east snapshot")
        .clone();
    let (west_owner, east_owner) = {
        let routes = handle
            .selected_read_routes
            .read()
            .expect("regional test lock poisoned");
        assert_ne!(routes[&west].lease.lane, routes[&east].lease.lane);
        (routes[&west].owner.clone(), routes[&east].owner.clone())
    };

    let (lane_entered, lane_entered_rx) = mpsc::channel();
    let (lane_release, lane_release_rx) = mpsc::channel();
    west_owner
        .hold_for_test(lane_entered, lane_release_rx)
        .expect("hold west owner worker");
    lane_entered_rx.recv().expect("west owner worker held");

    let (prepare_entered, prepare_entered_rx) = mpsc::channel();
    handle.notify_goal_prepare_entered_for_test(prepare_entered);
    let (prepare_complete, prepare_complete_rx) = mpsc::channel();
    let prepare_handle = handle.clone();
    let prepare_worker = std::thread::spawn(move || {
        prepare_complete
            .send(
                prepare_handle
                    .prepare_goal_tick_with_pathing_for_ids(17, Arc::new(HashSet::from([west]))),
            )
            .expect("publish goal preparation result");
    });
    prepare_entered_rx
        .recv()
        .expect("actor entered goal preparation");

    {
        let east_admission = east_owner.admission().try_lock();
        assert!(east_admission.is_ok());
    }
    assert!(
        handle
            .apply_kinematics_if_current([(
                east_snapshot,
                movement(east, Vec3::new(129.0, 64.0, 0.5)),
            )])
            .expect("east mutation during west goal preparation")
    );
    assert!(matches!(
        prepare_complete_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    lane_release.send(()).expect("release west owner worker");
    prepare_complete_rx
        .recv()
        .expect("goal preparation result")
        .expect("west goal preparation");
    prepare_worker.join().expect("join goal preparation");

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn goal_preparation_waits_for_same_lane_finalize_without_busy() {
    let (record_started, record_started_rx) = mpsc::sync_channel(0);
    let (record_release, record_release_rx) = mpsc::sync_channel(0);
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(BlockingRecordDecisionJournal {
            record_started,
            record_release: record_release_rx,
        }),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle
        .spawn_deferred_journal(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    let expected = handle
        .snapshot(entity)
        .expect("warm cow route")
        .expect("cow snapshot");

    let mutation_handle = handle.clone();
    let mutation = std::thread::spawn(move || {
        mutation_handle
            .apply_kinematics_if_current([(expected, movement(entity, Vec3::new(1.0, 64.0, 0.5)))])
    });
    record_started_rx
        .recv()
        .expect("direct mutation committed before finalize");

    let (prepare_entered, prepare_entered_rx) = mpsc::channel();
    handle.notify_goal_prepare_entered_for_test(prepare_entered);
    let (prepare_complete, prepare_complete_rx) = mpsc::channel();
    let prepare_handle = handle.clone();
    let prepare = std::thread::spawn(move || {
        prepare_complete
            .send(
                prepare_handle
                    .prepare_goal_tick_with_pathing_for_ids(18, Arc::new(HashSet::from([entity]))),
            )
            .expect("publish goal preparation");
    });
    prepare_entered_rx
        .recv()
        .expect("actor entered goal preparation");
    assert!(matches!(
        prepare_complete_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    record_release
        .send(())
        .expect("release direct journal record");
    assert_eq!(mutation.join().expect("join direct mutation"), Ok(true));
    prepare_complete_rx
        .recv()
        .expect("goal preparation result")
        .expect("goal preparation after finalize");
    prepare.join().expect("join goal preparation");

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn goal_apply_blocks_only_its_participant_lanes() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let snapshots = handle
        .snapshots_for_ids(&HashSet::from([west, east]))
        .expect("warm owner routes");
    let east_snapshot = snapshots
        .iter()
        .find(|snapshot| snapshot.id == east)
        .expect("east snapshot")
        .clone();
    let west_owner = {
        let routes = handle
            .selected_read_routes
            .read()
            .expect("regional test lock poisoned");
        assert_ne!(routes[&west].lease.lane, routes[&east].lease.lane);
        routes[&west].owner.clone()
    };
    let resolved = handle
        .prepare_goal_tick_with_pathing_for_ids(19, Arc::new(HashSet::from([west])))
        .expect("prepare west goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    let west_admission = west_owner
        .admission()
        .lock()
        .expect("regional test lock poisoned");
    let (apply_entered, apply_entered_rx) = mpsc::channel();
    handle.notify_goal_apply_topology_for_test(apply_entered);
    let (apply_complete, apply_complete_rx) = mpsc::channel();
    let apply_handle = handle.clone();
    let apply = std::thread::spawn(move || {
        apply_complete
            .send(apply_handle.apply_prepared_goal_tick(resolved))
            .expect("publish goal apply");
    });
    apply_entered_rx
        .recv()
        .expect("goal apply acquired topology fence");
    {
        let shared_topology = handle.mutation_gate.try_read();
        assert!(shared_topology.is_ok());
    }

    assert!(
        handle
            .apply_kinematics_if_current([(
                east_snapshot,
                movement(east, Vec3::new(129.0, 64.0, 0.5)),
            )])
            .expect("east mutation during west goal apply")
    );
    assert!(matches!(
        apply_complete_rx.try_recv(),
        Err(mpsc::TryRecvError::Empty)
    ));

    drop(west_admission);
    apply_complete_rx
        .recv()
        .expect("goal apply result")
        .expect("west goal apply");
    apply.join().expect("join goal apply");

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn goal_apply_scope_covers_batch_target_and_kinematics_lanes() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 3)
            .expect("owner coordinator");
    let active = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("active cow");
    let target = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("target cow");
    let kinematics = coordinator
        .spawn(cow(Vec3::new(256.5, 64.0, 0.5)))
        .expect("kinematics cow");
    assert!(
        coordinator
            .set_goal(
                active,
                GoalState::FollowTarget {
                    target,
                    speed: 0.25,
                },
            )
            .expect("follow target goal")
    );
    let active_region = coordinator.locations[&active];
    let target_region = coordinator.locations[&target];
    let kinematics_region = coordinator.locations[&kinematics];
    let lanes = [active_region, target_region, kinematics_region]
        .into_iter()
        .map(|key| coordinator.ownership.lease(key).expect("region lease").lane)
        .collect::<HashSet<_>>();
    assert_eq!(lanes.len(), 3);

    let mut resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids(20, &HashSet::from([active]))
        .expect("prepare active goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    assert!(resolved.batches.contains_key(&active_region));
    assert!(resolved.follow_target_sources.contains_key(&target));
    resolved.goal_inputs.remove(&active_region);
    resolved.leases.remove(&active_region);

    let (reply, _) = mpsc::channel();
    let command = super::RegionalOwnerCommand::ApplyPreparedGoalTickAndKinematics {
        resolved: Box::new(resolved),
        entities: Arc::new(HashSet::from([kinematics])),
        defer_journal: false,
        reply,
    };
    let scope = command.lane_scope().expect("goal apply scope");
    assert_eq!(
        super::owner_readers_for_scope(&coordinator, &scope)
            .expect("all participant routes")
            .len(),
        3
    );

    let active_lease = coordinator
        .ownership
        .lease(active_region)
        .expect("active lease");
    coordinator
        .ownership
        .unassign(active_lease)
        .expect("remove batch route");
    assert!(matches!(
        super::topology_access_for_command(&coordinator, &command),
        super::RegionalOwnerTopologyAccess::Exclusive
    ));
    assert_eq!(
        coordinator
            .ownership
            .assign(active_region, active_lease.lane)
            .expect("restore batch route"),
        active_lease
    );

    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn deferred_multi_lane_goal_rejection_does_not_commit_prepared_peer() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("owner coordinator");
    let west = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let west_region = coordinator.locations[&west];
    let east_region = coordinator.locations[&east];
    let west_lane = coordinator.ownership.lease(west_region).unwrap().lane;
    let east_lane = coordinator.ownership.lease(east_region).unwrap().lane;
    assert_ne!(west_lane, east_lane);
    let entities = HashSet::from([west, east]);
    let before = coordinator
        .snapshots_for_ids(&entities)
        .expect("snapshots before rejected apply");
    let versions_before = coordinator
        .lanes
        .iter()
        .map(|(&lane, owner)| (lane, owner.reader().state_version()))
        .collect::<BTreeMap<_, _>>();
    let mut resolved = coordinator
        .prepare_goal_tick_with_pathing_for_ids(21, &entities)
        .expect("prepare two-lane goal")
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    resolved
        .goal_overrides
        .entry(east_region)
        .or_default()
        .extend([(east, GoalState::Idle), (east, GoalState::Idle)]);

    assert!(matches!(
        coordinator.apply_prepared_goal_tick_and_kinematics_for_ids(resolved, &entities, false,),
        Ok(None)
    ));
    assert_eq!(
        coordinator
            .snapshots_for_ids(&entities)
            .expect("snapshots after rejected apply"),
        before
    );
    assert_eq!(
        coordinator.lanes[&west_lane].reader().state_version(),
        versions_before[&west_lane] + 1,
        "the valid peer must stop after prepare"
    );
    assert_eq!(
        coordinator.lanes[&east_lane].reader().state_version(),
        versions_before[&east_lane],
        "the rejected peer must not prepare"
    );

    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn cached_read_ignores_unrelated_regional_writer() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let west_snapshot = handle
        .snapshot(west)
        .expect("warm west route")
        .expect("west snapshot");
    handle
        .snapshot(east)
        .expect("warm east route")
        .expect("east snapshot");

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");

    let (read_entered, read_entered_rx) = mpsc::channel();
    let (read_release, read_release_rx) = mpsc::channel();
    handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
    let read_handle = handle.clone();
    let (read_complete, read_complete_rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        read_complete
            .send(read_handle.snapshot(east))
            .expect("publish cached read");
    });
    read_entered_rx
        .recv()
        .expect("east read dispatched to its owner lane");

    assert!(
        handle
            .apply_kinematics_if_current([(
                west_snapshot,
                movement(west, Vec3::new(1.0, 64.0, 0.5)),
            )])
            .expect("west mutation")
    );
    read_release.send(()).expect("release east read");
    let direct = read_complete_rx.recv_timeout(Duration::from_secs(1));
    coordinator_release.send(()).expect("release coordinator");
    reader.join().expect("join cached reader");

    assert_eq!(
        direct
            .expect("unrelated writer must not force coordinator fallback")
            .expect("cached point read")
            .expect("east snapshot")
            .id,
        east
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn cached_multi_lane_read_retries_after_overlapping_mutation() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let selected = HashSet::from([west, east]);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("warm direct routes")
            .len(),
        2
    );

    let (read_entered, read_entered_rx) = mpsc::channel();
    let (read_release, read_release_rx) = mpsc::channel();
    handle.pause_selected_read_after_dispatch_for_test(read_entered, read_release_rx);
    let read_handle = handle.clone();
    let reader = std::thread::spawn(move || read_handle.snapshots_for_ids(&selected));
    read_entered_rx
        .recv()
        .expect("selected read dispatched to both lanes");

    handle
        .set_velocities([
            (west, Vec3::new(0.1, 0.0, 0.0)),
            (east, Vec3::new(0.2, 0.0, 0.0)),
        ])
        .expect("overlapping regional mutation");
    read_release.send(()).expect("release selected read");
    let snapshots = reader
        .join()
        .expect("join selected reader")
        .expect("selected read fallback");
    assert_eq!(snapshots.len(), 2);
    assert_eq!(snapshots[0].id, west);
    assert_eq!(snapshots[0].velocity, Vec3::new(0.1, 0.0, 0.0));
    assert_eq!(snapshots[1].id, east);
    assert_eq!(snapshots[1].velocity, Vec3::new(0.2, 0.0, 0.0));

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn selected_read_route_refreshes_after_region_migration() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let entity = handle
        .spawn(cow(Vec3::new(127.5, 64.0, 0.5)))
        .expect("boundary cow");
    let selected = HashSet::from([entity]);
    let expected = handle
        .snapshots_for_ids(&selected)
        .expect("warm source route")
        .pop()
        .expect("source snapshot");
    assert!(
        handle
            .apply_kinematics_if_current([(
                expected,
                movement(entity, Vec3::new(128.5, 64.0, 0.5)),
            )])
            .expect("cross-region movement")
    );
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("fallback and refresh target route")[0]
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );

    let (coordinator_entered, coordinator_entered_rx) = mpsc::channel();
    let (coordinator_release, coordinator_release_rx) = mpsc::channel();
    handle
        .hold_coordinator_for_test(coordinator_entered, coordinator_release_rx)
        .expect("queue coordinator hold");
    coordinator_entered_rx
        .recv()
        .expect("coordinator entered hold");
    let snapshot = handle
        .read_cached_selected_entities(&selected)
        .expect("refreshed target route remains direct")
        .snapshots
        .pop()
        .expect("target snapshot");
    assert_eq!(snapshot.position, Vec3::new(128.5, 64.0, 0.5));

    coordinator_release.send(()).expect("release coordinator");
    handle.status().expect("coordinator resumed");
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_runtime_routes_goal_and_conditional_physics_commands() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.goal = GoalState::Wander {
        speed: 0.2,
        period_ticks: 20,
    };
    let west = handle.spawn(west).expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");

    let prepared = handle
        .prepare_goal_tick_with_pathing_for_ids(17, Arc::new(HashSet::from([west])))
        .expect("prepare runtime goal");
    let stats = handle
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("apply runtime goal");
    assert_eq!(stats.decisions_applied, 1);

    let expected = handle
        .snapshot(east)
        .expect("east read")
        .expect("east snapshot");
    assert!(
        handle
            .apply_kinematics_if_current([(expected, movement(east, Vec3::new(129.0, 64.0, 0.5)),)])
            .expect("runtime physics")
    );
    assert_eq!(
        handle
            .snapshot(east)
            .expect("moved read")
            .expect("moved snapshot")
            .position,
        Vec3::new(129.0, 64.0, 0.5)
    );
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_runtime_reloads_a_stale_versioned_goal_input() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let follower = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("follower");
    assert!(
        handle
            .set_goal(
                follower,
                GoalState::FollowPosition {
                    target: Vec3::new(4.5, 64.0, 0.5),
                    speed: 0.2,
                },
            )
            .expect("set follower goal")
    );
    let active = HashSet::from([follower]);
    let selected = handle
        .snapshots_for_ids_versioned(&active)
        .expect("versioned follower input");
    assert!(
        handle
            .set_position(follower, Vec3::new(1.5, 64.0, 0.5))
            .expect("move follower")
    );

    let prepared = handle
        .prepare_goal_tick_with_pathing_for_versioned_snapshots(
            17,
            Arc::new(active.clone()),
            selected,
        )
        .expect("prepare from stale input");
    let stats = handle
        .apply_prepared_goal_tick(prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT))
        .expect("apply fresh goal input");

    assert_eq!(stats.decisions_applied, 1);
    let snapshot = handle
        .snapshot(follower)
        .expect("follower read")
        .expect("follower snapshot");
    assert_eq!(snapshot.position, Vec3::new(1.5, 64.0, 0.5));
    assert_ne!(snapshot.velocity, Vec3::ZERO);
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn versioned_snapshot_fence_rejects_lane_reassignment() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("owner coordinator");
    coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west anchor");
    let selected_id = coordinator
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let snapshots = coordinator
        .snapshots_for_ids(&HashSet::from([selected_id]))
        .expect("selected snapshot");
    let fence = super::selected_version_fence(&coordinator, &snapshots).expect("version fence");
    let selected = super::VersionedEntitySnapshots {
        authority: coordinator.authority,
        fence,
        snapshots,
        publication_readers: None,
    };
    assert!(super::versioned_snapshots_are_current(
        &coordinator,
        &selected
    ));

    assert_eq!(coordinator.reconfigure_lanes(1), Ok(1));
    assert!(!super::versioned_snapshots_are_current(
        &coordinator,
        &selected
    ));

    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn owner_runtime_routes_common_gameplay_mutations() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let cow_id = handle.spawn(cow(Vec3::new(0.5, 64.0, 0.5))).expect("cow");
    assert!(
        handle
            .set_goal(
                cow_id,
                GoalState::FollowPosition {
                    target: Vec3::new(3.5, 64.0, 0.5),
                    speed: 0.25,
                },
            )
            .expect("set goal")
    );
    assert!(
        handle
            .set_position(cow_id, Vec3::new(1.5, 64.0, 0.5))
            .expect("set position")
    );
    assert_eq!(
        handle
            .damage(cow_id, damage_request(5.0))
            .expect("damage")
            .expect("damage result")
            .snapshot
            .health,
        15.0
    );

    let mut item = SpawnEntity::new(41, "minecraft:item", Vec3::new(2.5, 64.0, 0.5));
    item.item_stack = Some(crate::EntityItemStack::new(7, 1));
    let item = handle.spawn(item).expect("item");
    assert!(
        handle
            .set_item_stack(item, Some(crate::EntityItemStack::new(7, 2)))
            .expect("set item stack")
    );
    assert_eq!(
        handle
            .snapshot(item)
            .expect("item read")
            .expect("item snapshot")
            .item_stack,
        Some(crate::EntityItemStack::new(7, 2))
    );
    let herd = handle
        .spawn_batch([
            cow(Vec3::new(3.5, 64.0, 0.5)),
            cow(Vec3::new(128.5, 64.0, 0.5)),
        ])
        .expect("spawn herd");
    assert_eq!(herd.len(), 2);
    assert_eq!(
        handle
            .set_goals(herd.iter().copied().map(|entity| {
                (
                    entity,
                    GoalState::Wander {
                        speed: 0.15,
                        period_ticks: 40,
                    },
                )
            }))
            .expect("set herd goals"),
        2
    );
    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_unique_batch_skips_duplicate_input_and_restored_uuids() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
        .expect("owner runtime");
    let handle = runtime.handle();
    let restored_uuid = Uuid::from_u128(0x11);
    let new_uuid = Uuid::from_u128(0x22);
    let mut restored_store = EntityStore::new();
    let mut restored = cow(Vec3::new(0.5, 64.0, 0.5));
    restored.uuid = Some(restored_uuid);
    let restored_id = restored_store.spawn(restored);
    let restored = restored_store
        .snapshot(restored_id)
        .expect("restored snapshot");
    assert_eq!(handle.insert_snapshots_batch([restored.clone()]), Ok(1));

    let mut existing_duplicate = cow(Vec3::new(1.5, 64.0, 0.5));
    existing_duplicate.uuid = Some(restored_uuid);
    let mut new = cow(Vec3::new(2.5, 64.0, 0.5));
    new.uuid = Some(new_uuid);
    let mut input_duplicate = cow(Vec3::new(3.5, 64.0, 0.5));
    input_duplicate.uuid = Some(new_uuid);

    let committed = handle
        .spawn_unique_batch([existing_duplicate, new, input_duplicate])
        .expect("unique authoritative batch");

    assert_eq!(committed.len(), 1);
    assert_eq!(committed[0].uuid, new_uuid);
    assert_eq!(committed[0].position, Vec3::new(2.5, 64.0, 0.5));
    assert_eq!(handle.status().expect("owner status").entity_count, 2);
    assert_eq!(
        handle
            .snapshot(restored_id)
            .expect("restored read")
            .expect("restored entity"),
        restored
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_unique_batch_journals_once_and_returns_exact_durable_snapshots() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let runtime = super::RegionalOwnerRuntime::from_store_with_journal(
        RegionalEntityStore::new(),
        2,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner runtime");
    let handle = runtime.handle();
    handle
        .restore_checkpoint_boundary(37, 90)
        .expect("restore checkpoint boundary");
    assert_eq!(
        handle.advance_lifecycle_epoch(36),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );
    handle
        .restore_checkpoint_boundary(35, 80)
        .expect("older checkpoint cannot regress recovered owner boundary");
    let mut west = cow(Vec3::new(0.5, 64.0, 0.5));
    west.uuid = Some(Uuid::from_u128(0x33));
    let mut east = cow(Vec3::new(128.5, 64.0, 0.5));
    east.uuid = Some(Uuid::from_u128(0x44));

    let committed = handle
        .spawn_unique_batch([west, east])
        .expect("unique authoritative batch");

    let state = journal.lock().expect("journal state");
    assert_eq!(state.commits.len(), 1);
    let decision = state.commits[0].clone();
    assert_eq!(decision.upserts(), committed.as_slice());
    assert!(decision.removed().is_empty());
    assert_eq!(decision.lifecycle_epoch(), 37);
    assert!(decision.sequence_watermark() > 90);
    drop(state);
    let saved = handle.save_barrier().expect("owner save barrier");
    assert_eq!(saved.snapshots(), committed.as_slice());
    assert_eq!(saved.lifecycle_epoch(), 37);
    assert_eq!(saved.sequence_watermark(), decision.sequence_watermark());
    assert!(saved.journal_phases().contains(&decision.phase()));

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_unique_batch_outcome_unknown_enters_fail_stop_without_global_publication() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState {
        fail_record_unknown: true,
        ..TestDecisionJournalState::default()
    }));
    let mut coordinator = super::RegionalOwnerCoordinator::from_store_with_journal(
        RegionalEntityStore::new(),
        1,
        Box::new(TestDecisionJournal(Arc::clone(&journal))),
    )
    .expect("owner coordinator");
    coordinator
        .commit_state
        .advance_lifecycle_epoch(37)
        .expect("advance lifecycle epoch");
    let mut uncertain = cow(Vec3::new(0.5, 64.0, 0.5));
    uncertain.uuid = Some(Uuid::from_u128(0x55));

    assert_eq!(
        coordinator.spawn_unique_batch([uncertain]),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_eq!(coordinator.status().entity_count, 0);
    assert_eq!(
        coordinator.snapshots(),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    let mut rejected = cow(Vec3::new(1.5, 64.0, 0.5));
    rejected.uuid = Some(Uuid::from_u128(0x66));
    assert_eq!(
        coordinator.spawn_unique_batch([rejected]),
        Err(super::RegionOwnerLaneError::OutcomeUnknown)
    );
    let state = journal.lock().expect("journal state");
    assert_eq!(state.commits.len(), 1);
    assert_eq!(state.commits[0].upserts()[0].uuid, Uuid::from_u128(0x55));
    assert_eq!(state.commits[0].lifecycle_epoch(), 37);
    assert_eq!(
        coordinator
            .commit_state
            .journal_commits
            .lock()
            .expect("journal identities")
            .get(&state.commits[0].phase())
            .copied(),
        Some(state.commits[0].identity())
    );
}

#[test]
fn owner_runtime_exposes_cutover_reads_and_conditional_remove() {
    let runtime =
        super::RegionalOwnerRuntime::from_store(RegionalEntityStore::with_next_id(6_000), 2)
            .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    assert_eq!(west, EntityId(6_001));
    assert_eq!(east, EntityId(6_002));

    let selected = handle
        .snapshots_for_ids(&HashSet::from([east]))
        .expect("selected snapshots");
    assert_eq!(
        selected
            .iter()
            .map(|snapshot| snapshot.id)
            .collect::<Vec<_>>(),
        vec![east]
    );
    let status = handle.status().expect("owner status");
    assert_eq!(status.entity_count, 2);
    assert_eq!(status.lane_count, 2);
    assert!(
        handle
            .contains_uuid(
                handle
                    .snapshot(east)
                    .expect("east UUID read")
                    .expect("east UUID snapshot")
                    .uuid,
            )
            .expect("contains UUID")
    );

    let stale = handle
        .snapshot(west)
        .expect("west read")
        .expect("west snapshot");
    assert!(
        handle
            .set_position(west, Vec3::new(1.5, 64.0, 0.5))
            .expect("move west")
    );
    assert_eq!(
        handle
            .remove_if_current(stale)
            .expect("stale conditional remove"),
        None
    );
    let current = handle
        .snapshot(west)
        .expect("current west read")
        .expect("current west snapshot");
    assert_eq!(
        handle
            .remove_if_current(current.clone())
            .expect("fresh conditional remove"),
        Some(current)
    );
    assert_eq!(handle.status().expect("final owner status").entity_count, 1);

    let item = handle
        .spawn({
            let mut item = SpawnEntity::new(41, "minecraft:item", Vec3::new(2.5, 64.0, 0.5));
            item.item_stack = Some(crate::EntityItemStack::new(7, 4));
            item
        })
        .expect("item");
    let stale_item = handle
        .snapshot(item)
        .expect("item read")
        .expect("item snapshot");
    assert!(
        handle
            .set_item_stack(item, Some(crate::EntityItemStack::new(7, 3)))
            .expect("change item")
    );
    assert!(
        !handle
            .set_item_stack_if_current(stale_item, Some(crate::EntityItemStack::new(7, 2)),)
            .expect("stale item CAS")
    );
    let current_item = handle
        .snapshot(item)
        .expect("current item read")
        .expect("current item snapshot");
    assert!(
        handle
            .set_item_stack_if_current(current_item, Some(crate::EntityItemStack::new(7, 2)),)
            .expect("fresh item CAS")
    );
    let current_item = handle
        .snapshot(item)
        .expect("claim item read")
        .expect("claim item snapshot");
    let mut claimed_item = current_item.clone();
    claimed_item.retained.item_pickup_claim = Some(77);
    assert!(
        handle
            .replace_snapshot_if_current(current_item, claimed_item)
            .expect("install item pickup claim")
    );
    assert!(
        !handle
            .set_item_stack(item, Some(crate::EntityItemStack::new(7, 1)))
            .expect("claimed item stack mutation is rejected")
    );
    assert_eq!(
        handle
            .remove(item)
            .expect("claimed item removal is rejected"),
        None
    );
    let claimed_item = handle
        .snapshot(item)
        .expect("claimed item read")
        .expect("claimed item snapshot");
    let claimed_position = claimed_item.position;
    let mut forged_claim_resolution = claimed_item.clone();
    forged_claim_resolution.item_stack = Some(crate::EntityItemStack::new(7, 1));
    assert!(
        !handle
            .replace_snapshot_if_current(claimed_item, forged_claim_resolution)
            .expect("claimed item full-snapshot overwrite is rejected")
    );
    let moved_position = Vec3::new(2.75, 64.25, 0.5);
    assert!(
        !handle
            .set_position(item, moved_position)
            .expect("claimed item motion is rejected")
    );
    let resolved = handle
        .resolve_item_pickup_claim(item, 77, Some(crate::EntityItemStack::new(7, 4)))
        .expect("resolve item pickup claim")
        .expect("matching item pickup claim");
    let super::ItemPickupClaimResolution::Updated(resolved) = resolved else {
        panic!("partial item pickup claim must update the entity");
    };
    assert_eq!(resolved.position, claimed_position);
    assert_eq!(resolved.item_stack, Some(crate::EntityItemStack::new(7, 4)));
    assert_eq!(resolved.retained.item_pickup_claim, None);

    let mut vehicle = SpawnEntity::vehicle(
        VehicleKind::Boat,
        15,
        "minecraft:oak_boat",
        Vec3::new(3.5, 64.0, 0.5),
    );
    vehicle.vehicle.as_mut().expect("vehicle state").passenger = Some(EntityId(6_005));
    let mounted = handle
        .spawn_batch([vehicle, cow(Vec3::new(3.75, 64.0, 0.5))])
        .expect("mounted pair");
    let passenger = handle
        .snapshot(mounted[1])
        .expect("passenger read")
        .expect("passenger snapshot");
    assert_eq!(
        handle.remove_if_current(passenger),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );

    drop(handle);
    runtime.shutdown().expect("runtime shutdown");
}

#[test]
fn owner_runtime_reconfigures_lanes_through_the_actor() {
    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 1)
        .expect("owner runtime");
    let handle = runtime.handle();
    let west = handle
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("west cow");
    let east = handle
        .spawn(cow(Vec3::new(128.5, 64.0, 0.5)))
        .expect("east cow");
    let selected = HashSet::from([west, east]);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("warm single-lane routes")
            .len(),
        2
    );
    assert_eq!(
        handle
            .selected_read_routes
            .read()
            .expect("selected route cache")
            .len(),
        2
    );

    assert_eq!(handle.reconfigure_lanes(2), Ok(2));
    assert!(
        handle
            .selected_read_routes
            .read()
            .expect("cleared scale-up routes")
            .is_empty()
    );
    assert_eq!(handle.status().expect("status").lane_count, 2);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("selected snapshots")
            .len(),
        2
    );
    assert_eq!(handle.reconfigure_lanes(1), Ok(1));
    assert!(
        handle
            .selected_read_routes
            .read()
            .expect("cleared scale-down routes")
            .is_empty()
    );
    assert_eq!(handle.status().expect("status").lane_count, 1);
    assert_eq!(
        handle
            .snapshots_for_ids(&selected)
            .expect("selected snapshots after scale-down")
            .len(),
        2
    );

    runtime.shutdown().expect("runtime shutdown");
}

#[test]
#[ignore = "explicit release 1500v1500 regional entity battle benchmark"]
fn regional_owner_1500v1500_battle_benchmark_report() {
    const TEAM_SIZE: usize = 1_500;
    const TOTAL_ENTITIES: usize = TEAM_SIZE * 2;
    const FORMATION_WIDTH: usize = 50;
    const MAX_TICKS: u64 = 320;
    const MELEE_RANGE: f64 = 1.75;
    const ATTACK_PERIOD_TICKS: u64 = 10;
    const DEATH_REMOVE_DELAY_TICKS: u64 = 20;

    fn percentile(samples: &[u128], percentile: usize) -> u128 {
        samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
    }

    fn battle_order(ids: &[EntityId], seed: u64) -> Vec<EntityId> {
        fn mix(mut value: u64) -> u64 {
            value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
            value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
            value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
            value ^ (value >> 31)
        }
        let mut ordered = ids
            .iter()
            .copied()
            .enumerate()
            .map(|(index, id)| (mix(index as u64 ^ seed), id))
            .collect::<Vec<_>>();
        ordered.sort_unstable_by_key(|(key, _)| *key);
        ordered.into_iter().map(|(_, id)| id).collect()
    }

    fn formation_position(side_a: bool, index: usize) -> Vec3 {
        let rank = index / FORMATION_WIDTH;
        let file = index % FORMATION_WIDTH;
        let depth = rank as f64 * 0.8;
        let x = if side_a { -6.0 - depth } else { 6.0 + depth };
        let z = (file as f64 - (FORMATION_WIDTH as f64 - 1.0) * 0.5) * 0.9;
        Vec3::new(x, 64.0, z)
    }

    let lanes = std::env::var("SOLARIS_BATTLE_OWNER_LANES")
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .unwrap_or(6)
        .clamp(1, 6);
    #[cfg(target_os = "linux")]
    assert_benchmark_has_distinct_physical_cores(lanes);

    let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), lanes)
        .expect("battle owner runtime");
    let handle = runtime.handle();
    let mut spawns = Vec::with_capacity(TOTAL_ENTITIES);
    for index in 0..TEAM_SIZE {
        spawns.push(SpawnEntity::new(
            54,
            "minecraft:zombie",
            formation_position(true, index),
        ));
    }
    for index in 0..TEAM_SIZE {
        spawns.push(SpawnEntity::new(
            23,
            "minecraft:husk",
            formation_position(false, index),
        ));
    }
    let ids = handle.spawn_batch(spawns).expect("battle entities");
    assert_eq!(ids.len(), TOTAL_ENTITIES);
    let team_a = ids[..TEAM_SIZE].to_vec();
    let team_b = ids[TEAM_SIZE..].to_vec();
    let team_a_set = team_a.iter().copied().collect::<HashSet<_>>();
    let order_a = battle_order(&team_a, 0xA11C_E001);
    let order_b = battle_order(&team_b, 0xB47A_1E02);
    let order = order_a.iter().chain(&order_b).copied().collect::<Vec<_>>();
    let mut positions = HashMap::with_capacity(TOTAL_ENTITIES);
    for (index, &entity) in team_a.iter().enumerate() {
        positions.insert(entity, formation_position(true, index));
    }
    for (index, &entity) in team_b.iter().enumerate() {
        positions.insert(entity, formation_position(false, index));
    }
    let mut alive = ids.iter().copied().collect::<HashSet<_>>();
    let mut grid_a = BattleSpatialGrid::from_ids(team_a.iter().copied(), &positions);
    let mut grid_b = BattleSpatialGrid::from_ids(team_b.iter().copied(), &positions);
    let mut targets = HashMap::with_capacity(TOTAL_ENTITIES);
    let mut initial_goals = Vec::with_capacity(TOTAL_ENTITIES);
    for &actor in &team_a {
        let target = grid_b
            .choose_target(actor, &positions)
            .expect("team B target");
        targets.insert(actor, target);
        initial_goals.push((
            actor,
            GoalState::FollowTarget {
                target,
                speed: 0.27,
            },
        ));
    }
    for &actor in &team_b {
        let target = grid_a
            .choose_target(actor, &positions)
            .expect("team A target");
        targets.insert(actor, target);
        initial_goals.push((
            actor,
            GoalState::FollowTarget {
                target,
                speed: 0.27,
            },
        ));
    }
    assert_eq!(
        handle
            .set_goals_deferred_journal(initial_goals)
            .expect("install battle targets"),
        TOTAL_ENTITIES
    );

    let mut next_attack_tick = HashMap::with_capacity(TOTAL_ENTITIES);
    for index in 0..TEAM_SIZE {
        let phase = 1 + (index as u64 % ATTACK_PERIOD_TICKS);
        next_attack_tick.insert(team_a[index], phase);
        next_attack_tick.insert(team_b[index], phase);
    }
    let mut removals = BTreeMap::<u64, Vec<EntityId>>::new();
    let mut ticks_executed = 0_u64;
    let mut attacks_planned = 0_u64;
    let mut attacks_applied = 0_u64;
    let mut retargets = 0_u64;
    let mut deaths_a = 0_u64;
    let mut deaths_b = 0_u64;
    let mut missing_follow_targets = 0_u64;
    let mut tick_samples = Vec::<u128>::new();
    let mut goal_samples = Vec::<u128>::new();
    let mut movement_samples = Vec::<u128>::new();
    let mut attack_samples = Vec::<u128>::new();
    let mut retarget_samples = Vec::<u128>::new();
    let mut retarget_grid_samples = Vec::<u128>::new();
    let mut retarget_query_samples = Vec::<u128>::new();
    let mut retarget_commit_samples = Vec::<u128>::new();
    let battle_started = Instant::now();

    for tick in 1..=MAX_TICKS {
        if let Some(due) = removals.remove(&tick) {
            for entity in due {
                assert!(
                    targets.values().all(|target| *target != entity),
                    "removed battle entity must no longer be referenced as a target"
                );
                let _ = handle.remove(entity).expect("remove dead battle entity");
            }
        }
        if !team_a.iter().any(|entity| alive.contains(entity))
            || !team_b.iter().any(|entity| alive.contains(entity))
        {
            break;
        }
        let active = &alive;

        let tick_started = Instant::now();
        let goal_started = Instant::now();
        let prepared = handle
            .prepare_goal_tick_with_pathing_for_ids(tick, Arc::new(active.clone()))
            .expect("prepare battle goal tick");
        let resolved = prepared.resolve(&WalkablePathing, PathingBudget::DEFAULT);
        let (stats, selected, versioned) = handle
            .apply_prepared_goal_tick_and_kinematics_for_ids_deferred_journal(
                resolved,
                Arc::new(active.clone()),
            )
            .expect("apply battle goal tick and capture kinematics")
            .expect("fresh battle goal tick");
        missing_follow_targets += stats.missing_follow_targets as u64;
        goal_samples.push(goal_started.elapsed().as_micros());

        let movement_started = Instant::now();
        assert_eq!(
            selected.len(),
            alive.len(),
            "every live battle actor must be processed on every tick"
        );
        let mut movement_states = Vec::with_capacity(selected.len());
        let mut staged_positions = Vec::with_capacity(selected.len());
        for current in &selected {
            if !alive.contains(&current.id) {
                continue;
            }
            let Some(&target) = targets.get(&current.id) else {
                continue;
            };
            let Some(&target_position) = positions.get(&target) else {
                continue;
            };
            let dx = target_position.x - current.position.x;
            let dz = target_position.z - current.position.z;
            let distance = dx.hypot(dz);
            let max_step = 0.27;
            let step = (distance - MELEE_RANGE * 0.82).clamp(0.0, max_step);
            let (vx, vz) = if distance > f64::EPSILON && step > 0.0 {
                (dx / distance * max_step, dz / distance * max_step)
            } else {
                (0.0, 0.0)
            };
            let position = Vec3::new(
                current.position.x
                    + if distance > f64::EPSILON {
                        dx / distance * step
                    } else {
                        0.0
                    },
                current.position.y,
                current.position.z
                    + if distance > f64::EPSILON {
                        dz / distance * step
                    } else {
                        0.0
                    },
            );
            movement_states.push(EntityKinematics {
                id: current.id,
                position,
                rotation: current.rotation,
                velocity: Vec3::new(vx, 0.0, vz),
                on_ground: true,
            });
            staged_positions.push((current.id, position));
        }
        let versioned = versioned.expect("battle actor batch must be compact-fence eligible");
        let committed = handle
            .try_apply_kinematics_if_versioned_deferred_journal(versioned, movement_states)
            .expect("apply battle movement through compact fence")
            .expect("battle compact fence must remain current");
        assert_eq!(
            committed.len(),
            alive.len(),
            "battle movement batch must commit every live actor atomically"
        );
        positions.extend(staged_positions);
        movement_samples.push(movement_started.elapsed().as_micros());

        let attack_started = Instant::now();
        let mut damage_by_target = HashMap::<EntityId, f32>::new();
        for &attacker in active {
            if !alive.contains(&attacker)
                || tick < next_attack_tick.get(&attacker).copied().unwrap_or(1)
            {
                continue;
            }
            let Some(&target) = targets.get(&attacker) else {
                continue;
            };
            if !alive.contains(&target) {
                continue;
            }
            let attacker_position = positions[&attacker];
            let target_position = positions[&target];
            let dx = target_position.x - attacker_position.x;
            let dz = target_position.z - attacker_position.z;
            if dx * dx + dz * dz > MELEE_RANGE * MELEE_RANGE {
                continue;
            }
            attacks_planned += 1;
            next_attack_tick.insert(attacker, tick + ATTACK_PERIOD_TICKS);
            let damage = 3.5;
            damage_by_target
                .entry(target)
                .and_modify(|current| *current = current.max(damage))
                .or_insert(damage);
        }
        let target_ids = damage_by_target.keys().copied().collect::<HashSet<_>>();
        let expected_targets = handle
            .snapshots_for_ids(&target_ids)
            .expect("battle damage target snapshots");
        let damage_results = handle
            .damage_batch_if_current(expected_targets.into_iter().filter_map(|expected| {
                let amount = damage_by_target.get(&expected.id).copied()?;
                Some((
                    expected,
                    EntityDamageRequest {
                        amount,
                        tick,
                        death_remove_tick: tick + DEATH_REMOVE_DELAY_TICKS,
                        villager_gossip_event: None,
                    },
                ))
            }))
            .expect("apply battle melee damage batch");
        attacks_applied += damage_results.len() as u64;
        let mut killed = Vec::new();
        for damage in damage_results {
            let target = damage.snapshot.id;
            if damage.killed && alive.remove(&target) {
                targets.remove(&target);
                next_attack_tick.remove(&target);
                killed.push(target);
                removals
                    .entry(tick + DEATH_REMOVE_DELAY_TICKS)
                    .or_default()
                    .push(target);
                if team_a_set.contains(&target) {
                    deaths_a += 1;
                } else {
                    deaths_b += 1;
                }
            }
        }
        attack_samples.push(attack_started.elapsed().as_micros());

        let retarget_started = Instant::now();
        let mut retarget_grid_us = 0;
        let mut retarget_query_us = 0;
        let mut retarget_commit_us = 0;
        if !killed.is_empty() {
            let grid_started = Instant::now();
            grid_a = BattleSpatialGrid::from_ids(
                team_a
                    .iter()
                    .copied()
                    .filter(|entity| alive.contains(entity)),
                &positions,
            );
            grid_b = BattleSpatialGrid::from_ids(
                team_b
                    .iter()
                    .copied()
                    .filter(|entity| alive.contains(entity)),
                &positions,
            );
            retarget_grid_us = grid_started.elapsed().as_micros();

            let query_started = Instant::now();
            let mut goal_updates = Vec::new();
            for &actor in &order {
                if !alive.contains(&actor)
                    || targets
                        .get(&actor)
                        .is_some_and(|target| alive.contains(target))
                {
                    continue;
                }
                let target = if team_a_set.contains(&actor) {
                    grid_b.choose_target(actor, &positions)
                } else {
                    grid_a.choose_target(actor, &positions)
                };
                match target {
                    Some(target) => {
                        targets.insert(actor, target);
                        goal_updates.push((
                            actor,
                            GoalState::FollowTarget {
                                target,
                                speed: 0.27,
                            },
                        ));
                    }
                    None => {
                        targets.remove(&actor);
                        goal_updates.push((actor, GoalState::Idle));
                    }
                }
                retargets += 1;
            }
            retarget_query_us = query_started.elapsed().as_micros();
            if !goal_updates.is_empty() {
                let commit_started = Instant::now();
                let goal_update_count = goal_updates.len();
                assert_eq!(
                    handle
                        .set_goals_deferred_journal(goal_updates)
                        .expect("retarget battle entities"),
                    goal_update_count
                );
                retarget_commit_us = commit_started.elapsed().as_micros();
            }
        }
        retarget_grid_samples.push(retarget_grid_us);
        retarget_query_samples.push(retarget_query_us);
        retarget_commit_samples.push(retarget_commit_us);
        retarget_samples.push(retarget_started.elapsed().as_micros());
        tick_samples.push(tick_started.elapsed().as_micros());
        ticks_executed = tick;
    }

    assert_eq!(missing_follow_targets, 0);
    assert!(attacks_planned > 0);
    assert!(attacks_applied > 0);
    assert!(deaths_a + deaths_b > 0);
    assert!(retargets > 0);
    assert!(!tick_samples.is_empty());
    tick_samples.sort_unstable();
    goal_samples.sort_unstable();
    movement_samples.sort_unstable();
    attack_samples.sort_unstable();
    retarget_samples.sort_unstable();
    retarget_grid_samples.sort_unstable();
    retarget_query_samples.sort_unstable();
    retarget_commit_samples.sort_unstable();
    let alive_a = team_a
        .iter()
        .filter(|entity| alive.contains(entity))
        .count();
    let alive_b = team_b
        .iter()
        .filter(|entity| alive.contains(entity))
        .count();
    let elapsed_ms = battle_started.elapsed().as_millis();
    let report = format!(
        "{{\n  \"team_size\": {},\n  \"lanes\": {},\n  \"initial_actor_population\": {},\n  \"full_live_population_each_tick\": true,\n  \"ticks\": {},\n  \"elapsed_ms\": {},\n  \"alive_a\": {},\n  \"alive_b\": {},\n  \"deaths_a\": {},\n  \"deaths_b\": {},\n  \"attacks_planned\": {},\n  \"attacks_applied\": {},\n  \"retargets\": {},\n  \"missing_follow_targets\": {},\n  \"tick_p50_us\": {},\n  \"tick_p95_us\": {},\n  \"tick_p99_us\": {},\n  \"tick_max_us\": {},\n  \"goal_p99_us\": {},\n  \"movement_p99_us\": {},\n  \"attack_p99_us\": {},\n  \"retarget_p99_us\": {}\n}}\n",
        TEAM_SIZE,
        lanes,
        TOTAL_ENTITIES,
        ticks_executed,
        elapsed_ms,
        alive_a,
        alive_b,
        deaths_a,
        deaths_b,
        attacks_planned,
        attacks_applied,
        retargets,
        missing_follow_targets,
        percentile(&tick_samples, 50),
        percentile(&tick_samples, 95),
        percentile(&tick_samples, 99),
        tick_samples.last().copied().unwrap_or_default(),
        percentile(&goal_samples, 99),
        percentile(&movement_samples, 99),
        percentile(&attack_samples, 99),
        percentile(&retarget_samples, 99),
    );
    let report_path = std::env::var_os("SOLARIS_BATTLE_REPORT")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(".analysis/bench/entity-battle-1500x1500.json")
        });
    if let Some(parent) = report_path.parent() {
        std::fs::create_dir_all(parent).expect("create battle report directory");
    }
    std::fs::write(&report_path, report).expect("write battle report");
    let tick_p99_us = percentile(&tick_samples, 99);
    println!(
        "ENTITY_BATTLE_BENCH team_size={TEAM_SIZE} lanes={lanes} initial_actor_population={TOTAL_ENTITIES} full_live_population_each_tick=true ticks={ticks_executed} elapsed_ms={elapsed_ms} alive_a={alive_a} alive_b={alive_b} deaths_a={deaths_a} deaths_b={deaths_b} attacks_planned={attacks_planned} attacks_applied={attacks_applied} retargets={retargets} tick_p50_us={} tick_p95_us={} tick_p99_us={tick_p99_us} tick_max_us={} goal_p99_us={} movement_p99_us={} attack_p99_us={} retarget_p99_us={} report={}",
        percentile(&tick_samples, 50),
        percentile(&tick_samples, 95),
        tick_samples.last().copied().unwrap_or_default(),
        percentile(&goal_samples, 99),
        percentile(&movement_samples, 99),
        percentile(&attack_samples, 99),
        percentile(&retarget_samples, 99),
        report_path.display(),
    );
    println!(
        "ENTITY_BATTLE_RETARGET_PROFILE grid_p99_us={} query_p99_us={} commit_p99_us={}",
        percentile(&retarget_grid_samples, 99),
        percentile(&retarget_query_samples, 99),
        percentile(&retarget_commit_samples, 99),
    );
    assert!(
        tick_p99_us <= 50_000,
        "integrated 1500v1500 regional battle exceeded the frozen 50 ms tick target at p99: {tick_p99_us} us"
    );

    drop(handle);
    runtime.shutdown().expect("battle runtime shutdown");
}

#[test]
#[ignore = "explicit debug persistent regional owner scaling benchmark"]
fn persistent_owner_lane_scaling_benchmark_report() {
    const REGIONS: usize = 8;
    const ENTITIES_PER_REGION: usize = 256;
    const ITERATIONS: usize = 80;

    fn run(lanes: usize, active_regions: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), lanes)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entities = (0..REGIONS)
            .flat_map(|region| {
                (0..ENTITIES_PER_REGION).map(move |index| {
                    let x = region as f64 * 128.0 + 0.5 + (index % 16) as f64;
                    let z = 0.5 + (index / 16) as f64;
                    cow(Vec3::new(x, 64.0, z))
                })
            })
            .collect::<Vec<_>>();
        let ids = handle.spawn_batch(entities).expect("benchmark entities");
        let active_ids = ids
            .into_iter()
            .take(active_regions * ENTITIES_PER_REGION)
            .collect::<HashSet<_>>();

        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let expected = handle
                .snapshots_for_ids(&active_ids)
                .expect("benchmark active snapshots");
            let offset = if iteration % 2 == 0 { 0.01 } else { -0.01 };
            let states = expected.iter().cloned().map(|snapshot| {
                let mut state = movement(
                    snapshot.id,
                    Vec3::new(
                        snapshot.position.x + offset,
                        snapshot.position.y,
                        snapshot.position.z,
                    ),
                );
                state.rotation = snapshot.rotation;
                state.velocity = snapshot.velocity;
                state.on_ground = snapshot.on_ground;
                (snapshot, state)
            });
            let started = Instant::now();
            assert!(
                handle
                    .apply_kinematics_if_current(states)
                    .expect("benchmark kinematics")
            );
            samples.push(started.elapsed().as_micros());
        }
        let snapshots = handle.snapshots().expect("final snapshots");
        drop(handle);
        runtime.shutdown().expect("benchmark shutdown");
        (samples, snapshots)
    }

    fn percentile(samples: &[u128], percentile: usize) -> u128 {
        samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
    }

    let parallel_lanes = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(4);
    #[cfg(target_os = "linux")]
    assert_benchmark_has_distinct_physical_cores(parallel_lanes);
    let (mut serial, serial_state) = run(1, REGIONS);
    let (mut parallel, parallel_state) = run(parallel_lanes, REGIONS);
    assert_eq!(serial_state, parallel_state);
    serial.sort_unstable();
    parallel.sort_unstable();
    let parallel_p99_us = percentile(&parallel, 99);
    println!(
        "REGIONAL_OWNER_SCALING_BENCH entities={} regions={REGIONS} iterations={ITERATIONS} parallel_lanes={parallel_lanes} serial_p50_us={} serial_p95_us={} serial_p99_us={} serial_max_us={} parallel_p50_us={} parallel_p95_us={} parallel_p99_us={parallel_p99_us} parallel_max_us={}",
        REGIONS * ENTITIES_PER_REGION,
        percentile(&serial, 50),
        percentile(&serial, 95),
        percentile(&serial, 99),
        serial.last().copied().unwrap_or_default(),
        percentile(&parallel, 50),
        percentile(&parallel, 95),
        parallel.last().copied().unwrap_or_default(),
    );
    assert!(
        parallel_p99_us <= 50_000,
        "2048-entity regional owner exceeded the frozen 50 ms tick target at p99: {parallel_p99_us} us"
    );

    let active_regions = 2;
    let (mut active_serial, active_serial_state) = run(1, active_regions);
    let (mut active_parallel, active_parallel_state) = run(parallel_lanes, active_regions);
    assert_eq!(active_serial_state, active_parallel_state);
    active_serial.sort_unstable();
    active_parallel.sort_unstable();
    let active_parallel_p99_us = percentile(&active_parallel, 99);
    println!(
        "REGIONAL_OWNER_ACTIVE_SUBSET_BENCH entities={} active_entities={} regions={REGIONS} active_regions={active_regions} iterations={ITERATIONS} parallel_lanes={parallel_lanes} serial_p50_us={} serial_p95_us={} serial_p99_us={} serial_max_us={} parallel_p50_us={} parallel_p95_us={} parallel_p99_us={active_parallel_p99_us} parallel_max_us={}",
        REGIONS * ENTITIES_PER_REGION,
        active_regions * ENTITIES_PER_REGION,
        percentile(&active_serial, 50),
        percentile(&active_serial, 95),
        percentile(&active_serial, 99),
        active_serial.last().copied().unwrap_or_default(),
        percentile(&active_parallel, 50),
        percentile(&active_parallel, 95),
        active_parallel.last().copied().unwrap_or_default(),
    );
    assert!(
        active_parallel_p99_us <= 50_000,
        "512-active regional owner subset exceeded the frozen 50 ms tick target at p99: {active_parallel_p99_us} us"
    );
}

#[test]
#[ignore = "explicit debug regional coordinator actor overhead benchmark"]
fn regional_coordinator_actor_overhead_benchmark_report() {
    const REGIONS: usize = 2;
    const ENTITIES_PER_REGION: usize = 256;
    const ITERATIONS: usize = 80;

    fn entities() -> Vec<SpawnEntity> {
        (0..REGIONS)
            .flat_map(|region| {
                (0..ENTITIES_PER_REGION).map(move |index| {
                    let x = region as f64 * 128.0 + 0.5 + (index % 16) as f64;
                    let z = 0.5 + (index / 16) as f64;
                    cow(Vec3::new(x, 64.0, z))
                })
            })
            .collect()
    }

    fn moved(
        expected: Vec<crate::EntitySnapshot>,
        iteration: usize,
    ) -> Vec<(crate::EntitySnapshot, EntityKinematics)> {
        let offset = if iteration.is_multiple_of(2) {
            0.01
        } else {
            -0.01
        };
        expected
            .into_iter()
            .map(|snapshot| {
                let mut state = movement(
                    snapshot.id,
                    Vec3::new(
                        snapshot.position.x + offset,
                        snapshot.position.y,
                        snapshot.position.z,
                    ),
                );
                state.rotation = snapshot.rotation;
                state.velocity = snapshot.velocity;
                state.on_ground = snapshot.on_ground;
                (snapshot, state)
            })
            .collect()
    }

    fn direct(lanes: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let mut coordinator =
            super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), lanes)
                .expect("direct coordinator");
        coordinator
            .spawn_batch(entities())
            .expect("direct benchmark entities");
        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let expected = coordinator.snapshots().expect("direct snapshots");
            let started = Instant::now();
            assert!(
                coordinator
                    .apply_kinematics_if_current(moved(expected, iteration))
                    .expect("direct kinematics")
            );
            samples.push(started.elapsed().as_micros());
        }
        let snapshots = coordinator.snapshots().expect("direct final snapshots");
        coordinator.shutdown().expect("direct shutdown");
        (samples, snapshots)
    }

    fn raw_ecs() -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let mut store = EntityStore::new();
        store.spawn_batch(entities());
        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let expected = store.snapshots().collect::<Vec<_>>();
            let states = moved(expected, iteration)
                .into_iter()
                .map(|(_, state)| state)
                .collect::<Vec<_>>();
            let started = Instant::now();
            assert_eq!(
                store.apply_kinematics(states),
                REGIONS * ENTITIES_PER_REGION
            );
            samples.push(started.elapsed().as_micros());
        }
        (samples, store.snapshots().collect())
    }

    fn actor(lanes: usize) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), lanes)
            .expect("actor runtime");
        let handle = runtime.handle();
        handle
            .spawn_batch(entities())
            .expect("actor benchmark entities");
        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let expected = handle.snapshots().expect("actor snapshots");
            let started = Instant::now();
            assert!(
                handle
                    .apply_kinematics_if_current(moved(expected, iteration))
                    .expect("actor kinematics")
            );
            samples.push(started.elapsed().as_micros());
        }
        let snapshots = handle.snapshots().expect("actor final snapshots");
        drop(handle);
        runtime.shutdown().expect("actor shutdown");
        (samples, snapshots)
    }

    fn percentile(samples: &[u128], percentile: usize) -> u128 {
        samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
    }

    let lanes = std::thread::available_parallelism()
        .map_or(1, usize::from)
        .min(REGIONS);
    #[cfg(target_os = "linux")]
    assert_benchmark_has_distinct_physical_cores(lanes);
    let (mut raw_ecs, raw_ecs_state) = raw_ecs();
    let (mut direct, direct_state) = direct(lanes);
    let (mut actor, actor_state) = actor(lanes);
    assert_eq!(raw_ecs_state, direct_state);
    assert_eq!(direct_state, actor_state);
    raw_ecs.sort_unstable();
    direct.sort_unstable();
    actor.sort_unstable();
    println!(
        "REGIONAL_COORDINATOR_ACTOR_OVERHEAD_BENCH entities={} regions={REGIONS} iterations={ITERATIONS} lanes={lanes} raw_ecs_p50_us={} raw_ecs_p95_us={} raw_ecs_p99_us={} direct_p50_us={} direct_p95_us={} direct_p99_us={} actor_p50_us={} actor_p95_us={} actor_p99_us={}",
        REGIONS * ENTITIES_PER_REGION,
        percentile(&raw_ecs, 50),
        percentile(&raw_ecs, 95),
        percentile(&raw_ecs, 99),
        percentile(&direct, 50),
        percentile(&direct, 95),
        percentile(&direct, 99),
        percentile(&actor, 50),
        percentile(&actor, 95),
        percentile(&actor, 99),
    );
}

#[test]
#[ignore = "explicit debug cached animal mutation benchmark"]
fn cached_animal_mutation_benchmark_report() {
    const ENTITIES: usize = 128;
    const ITERATIONS: usize = 200;

    fn run(force_actor: bool) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entities = (0..ENTITIES).map(|index| {
            let mut entity = cow(Vec3::new(
                0.5 + (index % 16) as f64,
                64.0,
                0.5 + (index / 16) as f64,
            ));
            entity.animal = Some(AnimalBreedingState::adult());
            entity
        });
        let ids = handle
            .spawn_batch(entities)
            .expect("benchmark animals")
            .into_iter()
            .collect::<HashSet<_>>();
        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let states = handle
                .snapshots_for_ids(&ids)
                .expect("benchmark snapshots")
                .into_iter()
                .map(|snapshot| {
                    let mut animal = snapshot.animal.expect("animal state");
                    animal.love_ticks = if iteration.is_multiple_of(2) { 1 } else { 2 };
                    (snapshot, animal)
                })
                .collect::<Vec<_>>();
            if force_actor {
                handle
                    .selected_read_routes
                    .write()
                    .expect("benchmark route cache lock poisoned")
                    .clear();
            }
            let started = Instant::now();
            assert!(
                handle
                    .set_animal_states_if_current(states)
                    .expect("animal mutation")
            );
            samples.push(started.elapsed().as_micros());
        }
        let snapshots = handle.snapshots().expect("final snapshots");
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
        (samples, snapshots)
    }

    fn run_concurrent(force_actor: bool) -> (Vec<u128>, Vec<crate::EntitySnapshot>) {
        let runtime = super::RegionalOwnerRuntime::from_store(RegionalEntityStore::new(), 2)
            .expect("owner runtime");
        let handle = runtime.handle();
        let entities = (0..ENTITIES).map(|index| {
            let region = index % 2;
            let local = index / 2;
            let mut entity = cow(Vec3::new(
                region as f64 * 128.0 + 0.5 + (local % 8) as f64,
                64.0,
                0.5 + (local / 8) as f64,
            ));
            entity.animal = Some(AnimalBreedingState::adult());
            entity
        });
        let spawned = handle.spawn_batch(entities).expect("benchmark animals");
        let west = spawned.iter().step_by(2).copied().collect::<HashSet<_>>();
        let east = spawned
            .iter()
            .skip(1)
            .step_by(2)
            .copied()
            .collect::<HashSet<_>>();
        let mut samples = Vec::with_capacity(ITERATIONS);
        for iteration in 0..ITERATIONS {
            let build_states = |ids: &HashSet<EntityId>| {
                handle
                    .snapshots_for_ids(ids)
                    .expect("benchmark snapshots")
                    .into_iter()
                    .map(|snapshot| {
                        let mut animal = snapshot.animal.expect("animal state");
                        animal.love_ticks = if iteration.is_multiple_of(2) { 1 } else { 2 };
                        (snapshot, animal)
                    })
                    .collect::<Vec<_>>()
            };
            let west_states = build_states(&west);
            let east_states = build_states(&east);
            if force_actor {
                handle
                    .selected_read_routes
                    .write()
                    .expect("benchmark route cache lock poisoned")
                    .clear();
            }
            let barrier = Arc::new(std::sync::Barrier::new(3));
            let started = Instant::now();
            std::thread::scope(|scope| {
                let west_barrier = Arc::clone(&barrier);
                let west_handle = &handle;
                let west = scope.spawn(move || {
                    west_barrier.wait();
                    west_handle.set_animal_states_if_current(west_states)
                });
                let east_barrier = Arc::clone(&barrier);
                let east_handle = &handle;
                let east = scope.spawn(move || {
                    east_barrier.wait();
                    east_handle.set_animal_states_if_current(east_states)
                });
                barrier.wait();
                assert!(west.join().expect("west worker").expect("west mutation"));
                assert!(east.join().expect("east worker").expect("east mutation"));
            });
            samples.push(started.elapsed().as_micros());
        }
        let snapshots = handle.snapshots().expect("final snapshots");
        drop(handle);
        runtime.shutdown().expect("runtime shutdown");
        (samples, snapshots)
    }

    fn percentile(samples: &[u128], percentile: usize) -> u128 {
        samples[(samples.len() * percentile).div_ceil(100).saturating_sub(1)]
    }

    let (mut direct, direct_state) = run(false);
    let (mut actor, actor_state) = run(true);
    let (mut concurrent_direct, concurrent_direct_state) = run_concurrent(false);
    let (mut concurrent_actor, concurrent_actor_state) = run_concurrent(true);
    assert_eq!(direct_state, actor_state);
    assert_eq!(concurrent_direct_state, concurrent_actor_state);
    direct.sort_unstable();
    actor.sort_unstable();
    concurrent_direct.sort_unstable();
    concurrent_actor.sort_unstable();
    println!(
        "CACHED_ANIMAL_MUTATION_BENCH entities={ENTITIES} iterations={ITERATIONS} direct_p50_us={} direct_p95_us={} direct_p99_us={} actor_p50_us={} actor_p95_us={} actor_p99_us={} concurrent_direct_p50_us={} concurrent_direct_p95_us={} concurrent_direct_p99_us={} concurrent_actor_p50_us={} concurrent_actor_p95_us={} concurrent_actor_p99_us={}",
        percentile(&direct, 50),
        percentile(&direct, 95),
        percentile(&direct, 99),
        percentile(&actor, 50),
        percentile(&actor, 95),
        percentile(&actor, 99),
        percentile(&concurrent_direct, 50),
        percentile(&concurrent_direct, 95),
        percentile(&concurrent_direct, 99),
        percentile(&concurrent_actor, 50),
        percentile(&concurrent_actor, 95),
        percentile(&concurrent_actor, 99),
    );
}

#[test]
fn owner_selected_reads_reject_stale_coordinator_indexes() {
    let mut coordinator =
        super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 1)
            .expect("owner coordinator");
    let entity = coordinator
        .spawn(cow(Vec3::new(0.5, 64.0, 0.5)))
        .expect("cow");
    coordinator.uuids.clear();

    assert_eq!(
        coordinator.snapshots_for_ids(&HashSet::from([entity])),
        Err(super::RegionOwnerLaneError::InvalidMutation)
    );

    coordinator.shutdown().expect("coordinator shutdown");
}

#[test]
fn production_authority_moves_vehicle_and_passenger_as_one_group() {
    let mut authority = RegionalEntityAuthority::default();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = authority.spawn_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);
    assert_eq!(ids, vec![EntityId(1), EntityId(2)]);

    assert_eq!(
        authority.apply_kinematics([movement(ids[0], Vec3::new(128.25, 64.0, 0.5))]),
        1
    );

    let migrated_boat = authority.snapshot(ids[0]).expect("migrated boat");
    let migrated_passenger = authority.snapshot(ids[1]).expect("migrated passenger");
    assert_eq!(migrated_boat.position, Vec3::new(128.25, 64.0, 0.5));
    assert_eq!(
        migrated_boat.vehicle.and_then(|vehicle| vehicle.passenger),
        Some(ids[1])
    );
    assert_eq!(migrated_passenger.position, Vec3::new(128.5, 64.0, 0.5));
    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);

    assert!(authority.set_position(ids[0], Vec3::new(127.5, 64.0, 0.5)));
    assert!(authority.set_position(ids[0], Vec3::new(128.25, 64.0, 0.5)));
    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
    assert_eq!(
        authority
            .snapshot(ids[0])
            .and_then(|snapshot| snapshot.vehicle)
            .and_then(|vehicle| vehicle.passenger),
        Some(ids[1])
    );
}

#[test]
fn production_authority_ignores_stale_passenger_state_after_group_crossing() {
    let mut authority = RegionalEntityAuthority::default();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = authority.spawn_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);

    assert_eq!(
        authority.apply_kinematics([
            movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
            movement(ids[1], Vec3::new(127.75, 64.0, 0.5)),
        ]),
        2
    );

    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
    assert_eq!(
        authority.snapshot(ids[1]).expect("passenger").position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn parallel_local_apply_keeps_vehicle_boundary_transfer_atomic() {
    let mut authority = RegionalEntityAuthority::default();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = authority.spawn_batch([
        boat,
        cow(Vec3::new(127.75, 64.0, 0.5)),
        cow(Vec3::new(-0.5, 64.0, 0.5)),
        cow(Vec3::new(256.5, 64.0, 0.5)),
    ]);

    let applied = authority.apply_kinematics_parallel_authoritative(
        [
            movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
            movement(ids[1], Vec3::new(127.75, 64.0, 0.5)),
            movement(ids[2], Vec3::new(-0.25, 64.0, 0.5)),
            movement(ids[3], Vec3::new(256.75, 64.0, 0.5)),
        ],
        2,
    );
    assert_eq!(applied.len(), 4);
    assert_eq!(
        applied
            .iter()
            .find(|state| state.id == ids[1])
            .expect("authoritative passenger result")
            .position,
        Vec3::new(128.5, 64.0, 0.5)
    );

    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 0);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 2);
    assert_eq!(
        authority.snapshot(ids[1]).expect("passenger").position,
        Vec3::new(128.5, 64.0, 0.5)
    );
    assert_eq!(
        authority.snapshot(ids[2]).expect("west cow").position,
        Vec3::new(-0.25, 64.0, 0.5)
    );
    assert_eq!(
        authority.snapshot(ids[3]).expect("east cow").position,
        Vec3::new(256.75, 64.0, 0.5)
    );
}

#[test]
fn production_authority_uses_vehicle_leader_when_passenger_crossing_arrives_first() {
    let mut authority = RegionalEntityAuthority::default();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = authority.spawn_batch([boat, cow(Vec3::new(127.75, 64.0, 0.5))]);

    assert_eq!(
        authority.apply_kinematics([
            movement(ids[1], Vec3::new(129.75, 64.0, 0.5)),
            movement(ids[0], Vec3::new(128.25, 64.0, 0.5)),
        ]),
        2
    );

    assert_eq!(
        authority.snapshot(ids[0]).expect("boat").position,
        Vec3::new(128.25, 64.0, 0.5)
    );
    assert_eq!(
        authority.snapshot(ids[1]).expect("passenger").position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn production_authority_prioritizes_crossing_vehicle_when_passenger_has_lower_id() {
    let mut authority = RegionalEntityAuthority::default();
    let passenger = cow(Vec3::new(127.75, 64.0, 0.5));
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(1));
    let ids = authority.spawn_batch([passenger, boat]);

    assert_eq!(
        authority.apply_kinematics([
            movement(ids[0], Vec3::new(0.5, 64.0, 0.5)),
            movement(ids[1], Vec3::new(128.25, 64.0, 0.5)),
        ]),
        2
    );

    assert_eq!(
        authority.snapshot(ids[1]).expect("boat").position,
        Vec3::new(128.25, 64.0, 0.5)
    );
    assert_eq!(
        authority.snapshot(ids[0]).expect("passenger").position,
        Vec3::new(128.5, 64.0, 0.5)
    );
}

#[test]
fn production_authority_rejects_vehicle_group_split_without_partial_move() {
    let mut authority = RegionalEntityAuthority::default();
    let mut boat = SpawnEntity::vehicle(
        VehicleKind::Boat,
        0,
        "minecraft:oak_boat",
        Vec3::new(127.5, 64.0, 0.5),
    );
    boat.vehicle.as_mut().expect("boat state").passenger = Some(EntityId(2));
    let ids = authority.spawn_batch([boat, cow(Vec3::new(0.5, 64.0, 0.5))]);
    let before = authority.snapshots().collect::<Vec<_>>();

    assert_eq!(
        authority.apply_kinematics([movement(ids[0], Vec3::new(128.25, 64.0, 0.5))]),
        0
    );
    assert_eq!(authority.snapshots().collect::<Vec<_>>(), before);
    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 2);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 0);
}

#[test]
fn production_authority_resolves_follow_target_across_region_boundary() {
    let mut authority = RegionalEntityAuthority::default();
    let target = cow(Vec3::new(127.5, 64.0, 0.5));
    let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target: EntityId(1),
        speed: 0.25,
    };
    let ids = authority.spawn_batch([target, follower]);

    assert!(authority.set_position(ids[0], Vec3::new(128.5, 64.0, 0.5)));
    let active = HashSet::from([ids[1]]);
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(23, &active)
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);
    let stats = authority.apply_prepared_goal_tick(resolved);

    assert_eq!(stats.missing_follow_targets, 0);
    assert_eq!(authority.region_len(RegionKey::new(0, 0)), 1);
    assert_eq!(authority.region_len(RegionKey::new(1, 0)), 1);
    let motion = authority.motion_state(ids[1]).expect("follower motion");
    assert!(motion.velocity.x > 0.0);
    assert_eq!(motion.velocity.z, 0.0);
    assert_eq!(
        authority.snapshot(ids[1]).expect("follower").goal,
        GoalState::FollowTarget {
            target: ids[0],
            speed: 0.25,
        }
    );
}

#[test]
fn production_authority_rejects_follow_batch_when_local_target_migrates() {
    let mut authority = RegionalEntityAuthority::default();
    let target = cow(Vec3::new(127.5, 64.0, 0.5));
    let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target: EntityId(1),
        speed: 0.25,
    };
    let ids = authority.spawn_batch([target, follower]);
    let active = HashSet::from([ids[1]]);
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(24, &active)
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    assert!(authority.set_position(ids[0], Vec3::new(128.5, 64.0, 0.5)));
    let stats = authority.apply_prepared_goal_tick(resolved);

    assert_eq!(stats.decisions_applied, 0);
    assert_eq!(stats.missing_follow_targets, 0);
    assert_eq!(
        authority.motion_state(ids[1]).expect("follower").velocity,
        Vec3::ZERO
    );
}

#[test]
fn production_authority_rejects_follow_batch_when_remote_target_moves() {
    let mut authority = RegionalEntityAuthority::default();
    let target = cow(Vec3::new(128.5, 64.0, 0.5));
    let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target: EntityId(1),
        speed: 0.25,
    };
    let ids = authority.spawn_batch([target, follower]);
    let active = HashSet::from([ids[1]]);
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(25, &active)
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    assert!(authority.set_position(ids[0], Vec3::new(129.5, 64.0, 0.5)));
    let stats = authority.apply_prepared_goal_tick(resolved);

    assert_eq!(stats.decisions_applied, 0);
    assert_eq!(stats.missing_follow_targets, 0);
    assert_eq!(
        authority.motion_state(ids[1]).expect("follower").velocity,
        Vec3::ZERO
    );
}

#[test]
fn production_authority_rejects_follow_batch_when_follower_goal_changes() {
    let mut authority = RegionalEntityAuthority::default();
    let target = cow(Vec3::new(128.5, 64.0, 0.5));
    let mut follower = cow(Vec3::new(126.5, 64.0, 0.5));
    follower.goal = GoalState::FollowTarget {
        target: EntityId(1),
        speed: 0.25,
    };
    let ids = authority.spawn_batch([target, follower]);
    let active = HashSet::from([ids[1]]);
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(26, &active)
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    assert!(authority.set_goal(ids[1], GoalState::Idle));
    assert!(authority.set_velocity(ids[1], Vec3::new(0.4, 0.0, 0.0)));
    let stats = authority.apply_prepared_goal_tick(resolved);

    assert_eq!(stats.decisions_applied, 0);
    assert_eq!(
        authority.snapshot(ids[1]).expect("follower").goal,
        GoalState::Idle
    );
    assert_eq!(
        authority.motion_state(ids[1]).expect("follower").velocity,
        Vec3::new(0.4, 0.0, 0.0)
    );
}

#[test]
fn production_authority_rejects_idle_batch_when_motion_changes() {
    let mut authority = RegionalEntityAuthority::default();
    let id = authority.spawn(cow(Vec3::new(0.5, 64.0, 0.5)));
    assert!(authority.set_velocity(id, Vec3::new(0.25, 0.0, 0.0)));
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(27, &HashSet::from([id]))
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    assert!(authority.set_velocity(id, Vec3::new(0.5, 0.0, 0.0)));
    let stats = authority.apply_prepared_goal_tick(resolved);

    assert_eq!(stats, GoalTickStats::default());
    assert_eq!(
        authority.motion_state(id).expect("idle cow").velocity,
        Vec3::new(0.5, 0.0, 0.0)
    );
}

#[test]
fn parallel_goal_apply_validates_all_regions_before_mutation() {
    let mut authority = RegionalEntityAuthority::default();
    let ids = authority.spawn_batch([
        cow(Vec3::new(-0.5, 64.0, 0.5)),
        cow(Vec3::new(128.5, 64.0, 0.5)),
    ]);
    assert!(authority.set_velocity(ids[0], Vec3::new(0.25, 0.0, 0.0)));
    assert!(authority.set_velocity(ids[1], Vec3::new(0.25, 0.0, 0.0)));
    let resolved = authority
        .prepare_goal_tick_with_pathing_for_ids(28, &ids.iter().copied().collect())
        .resolve(&WalkablePathing, PathingBudget::DEFAULT);

    assert!(authority.set_velocity(ids[0], Vec3::new(0.5, 0.0, 0.0)));
    let stats = authority.apply_prepared_goal_tick_parallel(resolved, 2);

    assert_eq!(stats, GoalTickStats::default());
    assert_eq!(
        authority.motion_state(ids[0]).expect("west cow").velocity,
        Vec3::new(0.5, 0.0, 0.0)
    );
    assert_eq!(
        authority.motion_state(ids[1]).expect("east cow").velocity,
        Vec3::new(0.25, 0.0, 0.0)
    );
}

#[test]
fn production_authority_closes_phase_when_mutation_panics() {
    let mut authority = RegionalEntityAuthority::default();
    let duplicate_uuid = Uuid::from_u128(5001);
    let mut first = cow(Vec3::new(0.5, 64.0, 0.5));
    first.uuid = Some(duplicate_uuid);
    authority.spawn(first);
    let mut duplicate = cow(Vec3::new(1.5, 64.0, 0.5));
    duplicate.uuid = Some(duplicate_uuid);

    let panic =
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| authority.spawn(duplicate)));
    assert!(panic.is_err());

    let recovered = authority.spawn(cow(Vec3::new(2.5, 64.0, 0.5)));
    assert!(authority.contains(recovered));
}

#[cfg(any())]
mod bread_only_legacy_population_tests {
    use super::*;

    fn population_villager(position: Vec3) -> SpawnEntity {
        let mut villager = SpawnEntity::new(119, "minecraft:villager", position);
        villager.retained.villager_population =
            Some(crate::villager_population_26_1_2::VillagerPopulationState::adult());
        villager
    }

    fn bread_item(position: Vec3, count: i32) -> SpawnEntity {
        let mut item = SpawnEntity::new(55, "minecraft:item", position);
        item.item_stack = Some(EntityItemStack::new(7, count));
        item
    }

    fn villager_courtship_commit(
        coordinator: &mut super::RegionalOwnerCoordinator,
        parents: [EntityId; 2],
        bread_items: [EntityId; 2],
    ) -> super::VillagerCourtshipCommit {
        let parent_expected = parents.map(|id| {
            coordinator
                .snapshot(id)
                .expect("parent read")
                .expect("parent snapshot")
        });
        let mut parent_next = parent_expected.clone();
        for index in 0..2 {
            let partner_uuid = parent_expected[1 - index].uuid;
            let population = parent_next[index]
                .retained
                .villager_population
                .as_mut()
                .expect("villager population");
            population.add_bread(3).expect("three bread");
            population
                .start_pending_birth(
                    partner_uuid,
                    "settlement:vacant-home".to_owned(),
                    10,
                    0,
                    false,
                )
                .expect("pending birth");
        }
        let first_bread = coordinator
            .snapshot(bread_items[0])
            .expect("first bread read")
            .expect("first bread snapshot");
        let second_bread = coordinator
            .snapshot(bread_items[1])
            .expect("second bread read")
            .expect("second bread snapshot");
        let mut second_bread_next = second_bread.clone();
        second_bread_next.item_stack = Some(EntityItemStack::new(7, 1));

        super::VillagerCourtshipCommit {
            parents: [
                (parent_expected[0].clone(), parent_next[0].clone()),
                (parent_expected[1].clone(), parent_next[1].clone()),
            ],
            bread_items: vec![(first_bread, None), (second_bread, Some(second_bread_next))],
            current_tick: 10,
            bread_item_id: 7,
            home_claim: "settlement:vacant-home".to_owned(),
            deterministic_seed: 0,
        }
    }

    fn villager_birth_commit(
        coordinator: &mut super::RegionalOwnerCoordinator,
        parents: [EntityId; 2],
        child_id: EntityId,
    ) -> super::VillagerBirthCommit {
        let parent_expected = parents.map(|id| {
            coordinator
                .snapshot(id)
                .expect("parent read")
                .expect("pending parent snapshot")
        });
        let parent_next = parent_expected.clone().map(|mut snapshot| {
            let population = snapshot
                .retained
                .villager_population
                .as_mut()
                .expect("villager population");
            population
                .finish_parent_birth("settlement:vacant-home", 285)
                .expect("due parent birth");
            snapshot
        });
        let mut child = SpawnEntity::new(
            parent_expected[0].type_id,
            "minecraft:villager",
            Vec3::new(128.0, 64.0, 0.5),
        );
        child.uuid = Some(Uuid::from_u128(child_id.0 as u128 + 10_000));
        child.retained.villager = parent_expected[0].retained.villager;
        child.position = Vec3::new(128.0, 64.0, 0.5);
        child.retained.villager_population = Some(
            crate::villager_population_26_1_2::VillagerPopulationState::baby(
                "settlement:vacant-home".to_owned(),
            ),
        );

        super::VillagerBirthCommit {
            parents: [
                (parent_expected[0].clone(), parent_next[0].clone()),
                (parent_expected[1].clone(), parent_next[1].clone()),
            ],
            child,
            current_tick: 285,
        }
    }

    fn seeded_courtship_coordinator(
        journal: Option<Arc<Mutex<TestDecisionJournalState>>>,
    ) -> (
        super::RegionalOwnerCoordinator,
        [EntityId; 2],
        [EntityId; 2],
    ) {
        let mut coordinator = match journal {
            Some(journal) => super::RegionalOwnerCoordinator::from_store_with_journal(
                RegionalEntityStore::new(),
                2,
                Box::new(TestDecisionJournal(journal)),
            )
            .expect("journaled owner coordinator"),
            None => super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
                .expect("owner coordinator"),
        };
        let parents = [
            coordinator
                .spawn(population_villager(Vec3::new(127.5, 64.0, 0.5)))
                .expect("west parent"),
            coordinator
                .spawn(population_villager(Vec3::new(128.5, 64.0, 0.5)))
                .expect("east parent"),
        ];
        let bread_items = [
            coordinator
                .spawn(bread_item(Vec3::new(127.75, 64.0, 0.5), 3))
                .expect("west bread"),
            coordinator
                .spawn(bread_item(Vec3::new(128.25, 64.0, 0.5), 4))
                .expect("east bread"),
        ];
        (coordinator, parents, bread_items)
    }

    fn seeded_birth_coordinator(
        journal: Option<Arc<Mutex<TestDecisionJournalState>>>,
    ) -> (super::RegionalOwnerCoordinator, [EntityId; 2]) {
        let (mut coordinator, parents, bread_items) = seeded_courtship_coordinator(journal);
        let courtship = villager_courtship_commit(&mut coordinator, parents, bread_items);
        assert!(
            coordinator
                .commit_villager_courtship_if_current(courtship)
                .expect("seed courtship")
        );
        (coordinator, parents)
    }

    fn member_snapshots(
        coordinator: &mut super::RegionalOwnerCoordinator,
        ids: impl IntoIterator<Item = EntityId>,
    ) -> Vec<(EntityId, crate::EntitySnapshot)> {
        ids.into_iter()
            .map(|id| {
                (
                    id,
                    coordinator
                        .snapshot(id)
                        .expect("member read")
                        .expect("member snapshot"),
                )
            })
            .collect()
    }

    fn assert_members_unchanged(
        coordinator: &mut super::RegionalOwnerCoordinator,
        before: Vec<(EntityId, crate::EntitySnapshot)>,
    ) {
        for (id, expected) in before {
            assert_eq!(
                coordinator
                    .snapshot(id)
                    .expect("post-commit read")
                    .expect("member conserved"),
                expected
            );
        }
    }

    #[test]
    fn villager_courtship_atomically_consumes_bread_and_persists_reciprocal_pending_state() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let (mut coordinator, parents, bread_items) =
            seeded_courtship_coordinator(Some(Arc::clone(&journal)));
        let commit = villager_courtship_commit(&mut coordinator, parents, bread_items);

        assert!(
            coordinator
                .commit_villager_courtship_if_current(commit)
                .expect("courtship commit")
        );
        for parent in parents {
            let population = coordinator
                .snapshot(parent)
                .expect("parent read")
                .expect("parent remains")
                .retained
                .villager_population
                .expect("parent population");
            assert_eq!(population.food_points, 12);
            let pending = population.pending_birth.expect("pending birth");
            assert_eq!(pending.started_tick, 10);
            assert_eq!(pending.ready_tick, 285);
            assert_eq!(pending.home_claim, "settlement:vacant-home");
        }
        assert!(
            coordinator
                .snapshot(bread_items[0])
                .expect("removed bread read")
                .is_none()
        );
        assert_eq!(
            coordinator
                .snapshot(bread_items[1])
                .expect("remaining bread read")
                .expect("remaining bread")
                .item_stack,
            Some(EntityItemStack::new(7, 1))
        );
        let state = journal.lock().expect("journal state");
        let decision = state.commits.last().expect("courtship decision");
        assert!(
            decision
                .upserts()
                .iter()
                .filter(|snapshot| {
                    snapshot
                        .retained
                        .villager_population
                        .as_ref()
                        .is_some_and(|population| population.food_points == 12)
                })
                .count()
                == 2
        );
        assert!(decision.removed().contains(&bread_items[0]));
    }

    #[test]
    fn villager_courtship_rejects_stale_parent_or_item_without_partial_mutation() {
        for stale_parent in [true, false] {
            let (mut coordinator, parents, bread_items) = seeded_courtship_coordinator(None);
            let commit = villager_courtship_commit(&mut coordinator, parents, bread_items);
            let stale_id = if stale_parent {
                parents[0]
            } else {
                bread_items[0]
            };
            coordinator
                .set_velocities([(stale_id, Vec3::new(0.25, 0.0, 0.0))])
                .expect("make courtship member stale");
            let before = member_snapshots(&mut coordinator, parents.into_iter().chain(bread_items));

            assert!(
                !coordinator
                    .commit_villager_courtship_if_current(commit)
                    .expect("stale courtship is a rejected compare")
            );
            assert_members_unchanged(&mut coordinator, before);
        }
    }

    #[test]
    fn villager_courtship_rejects_malformed_food_parent_home_distance_and_lifecycle_inputs() {
        for case in 0..9 {
            let (mut coordinator, parents, bread_items) = seeded_courtship_coordinator(None);
            let mut commit = villager_courtship_commit(&mut coordinator, parents, bread_items);
            match case {
                0 => commit.bread_item_id = 8,
                1 => {
                    commit.bread_items[1]
                        .1
                        .as_mut()
                        .expect("remaining bread")
                        .item_stack = Some(EntityItemStack::new(7, 2));
                }
                2 => commit.bread_items[1].1 = None,
                3 => {
                    commit.bread_items[1]
                        .1
                        .as_mut()
                        .expect("remaining bread")
                        .retained
                        .spawn_tick += 1;
                }
                4 => {
                    commit.parents[0]
                        .0
                        .retained
                        .villager_population
                        .as_mut()
                        .expect("adult population")
                        .age_ticks = 1;
                }
                5 => commit.home_claim.clear(),
                6 => {
                    let far = Vec3::new(256.0, 64.0, 0.5);
                    commit.parents[1].0.position = far;
                    commit.parents[1].1.position = far;
                }
                7 => {
                    let far = Vec3::new(64.0, 64.0, 0.5);
                    commit.bread_items[0].0.position = far;
                }
                8 => commit.bread_items[0].0.lifecycle = EntityLifecycle::Despawning,
                _ => unreachable!(),
            }
            let before = member_snapshots(&mut coordinator, parents.into_iter().chain(bread_items));
            assert_eq!(
                coordinator.commit_villager_courtship_if_current(commit),
                Err(super::RegionOwnerLaneError::InvalidMutation)
            );
            assert_members_unchanged(&mut coordinator, before);
        }
    }

    #[test]
    fn villager_courtship_safe_journal_failure_rolls_back_parents_and_items() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let (mut coordinator, parents, bread_items) =
            seeded_courtship_coordinator(Some(Arc::clone(&journal)));
        let commit = villager_courtship_commit(&mut coordinator, parents, bread_items);
        let before = member_snapshots(&mut coordinator, parents.into_iter().chain(bread_items));
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            coordinator.commit_villager_courtship_if_current(commit),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_members_unchanged(&mut coordinator, before);
    }

    #[test]
    fn villager_birth_atomically_finishes_parents_and_inserts_claimed_child() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let (mut coordinator, parents) = seeded_birth_coordinator(Some(Arc::clone(&journal)));
        let commit = villager_birth_commit(&mut coordinator, parents, EntityId(900));
        let replay = commit.clone();
        let child_uuid = commit.child.uuid.expect("stable child uuid");
        let next_id_before = coordinator.next_id;

        let committed_child = coordinator
            .commit_villager_birth_if_current(commit)
            .expect("birth commit")
            .expect("authoritative child");
        assert_eq!(committed_child.id, EntityId(next_id_before + 1));
        assert_eq!(committed_child.uuid, child_uuid);
        for parent in parents {
            let population = coordinator
                .snapshot(parent)
                .expect("parent read")
                .expect("parent remains")
                .retained
                .villager_population
                .expect("parent population");
            assert_eq!(population.food_points, 0);
            assert_eq!(
                population.age_ticks,
                crate::villager_population_26_1_2::VILLAGER_PARENT_COOLDOWN_TICKS
            );
            assert!(population.pending_birth.is_none());
        }
        let child = coordinator
            .snapshot(committed_child.id)
            .expect("child read")
            .expect("child inserted");
        assert_eq!(child, committed_child);
        assert_eq!(
            child
                .retained
                .villager_population
                .expect("child population")
                .claimed_home
                .as_deref(),
            Some("settlement:vacant-home")
        );
        let next_id_after_birth = coordinator.next_id;
        assert!(
            coordinator
                .commit_villager_birth_if_current(replay)
                .expect("birth replay is a rejected compare")
                .is_none()
        );
        assert_eq!(coordinator.next_id, next_id_after_birth);
        let state = journal.lock().expect("journal state");
        let decision = state.commits.last().expect("birth decision");
        assert!(
            decision
                .upserts()
                .iter()
                .any(|snapshot| snapshot.id == committed_child.id)
        );
    }

    #[test]
    fn villager_birth_rejects_stale_parent_without_partial_mutation() {
        let (mut coordinator, parents) = seeded_birth_coordinator(None);
        let commit = villager_birth_commit(&mut coordinator, parents, EntityId(901));
        let child_uuid = commit.child.uuid.expect("stable child uuid");
        let next_id_before = coordinator.next_id;
        coordinator
            .set_velocities([(parents[0], Vec3::new(0.25, 0.0, 0.0))])
            .expect("make parent stale");
        let before = member_snapshots(&mut coordinator, parents);

        assert!(
            coordinator
                .commit_villager_birth_if_current(commit)
                .expect("stale birth compare")
                .is_none()
        );
        assert_members_unchanged(&mut coordinator, before);
        assert_eq!(coordinator.next_id, next_id_before);
        assert!(!coordinator.uuids.contains_key(&child_uuid));
    }

    #[test]
    fn villager_birth_safe_journal_failure_rolls_back_parents_and_child() {
        let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
        let (mut coordinator, parents) = seeded_birth_coordinator(Some(Arc::clone(&journal)));
        let commit = villager_birth_commit(&mut coordinator, parents, EntityId(902));
        let child_uuid = commit.child.uuid.expect("stable child uuid");
        let next_id_before = coordinator.next_id;
        let before = member_snapshots(&mut coordinator, parents);
        journal.lock().expect("journal state").fail_record = true;

        assert_eq!(
            coordinator.commit_villager_birth_if_current(commit),
            Err(super::RegionOwnerLaneError::Journal)
        );
        assert_members_unchanged(&mut coordinator, before);
        assert_eq!(coordinator.next_id, next_id_before);
        assert!(!coordinator.uuids.contains_key(&child_uuid));
    }

    #[test]
    fn villager_birth_rejects_not_due_nonreciprocal_and_malformed_child() {
        for case in 0..5 {
            let (mut coordinator, parents) = seeded_birth_coordinator(None);
            let mut commit = villager_birth_commit(&mut coordinator, parents, EntityId(910 + case));
            match case {
                0 => commit.current_tick = 284,
                1 => {
                    commit.parents[0]
                        .0
                        .retained
                        .villager_population
                        .as_mut()
                        .expect("parent population")
                        .pending_birth
                        .as_mut()
                        .expect("pending birth")
                        .partner_uuid = Uuid::from_u128(999_999);
                }
                2 => {
                    commit
                        .child
                        .retained
                        .villager_population
                        .as_mut()
                        .expect("child population")
                        .claimed_home = Some("settlement:wrong-home".to_owned());
                }
                3 => commit.child.uuid = None,
                4 => commit.child.uuid = Some(commit.parents[0].0.uuid),
                _ => unreachable!(),
            }
            let next_id_before = coordinator.next_id;
            let uuids_before = coordinator.uuids.clone();
            let before = member_snapshots(&mut coordinator, parents);
            assert_eq!(
                coordinator.commit_villager_birth_if_current(commit),
                Err(super::RegionOwnerLaneError::InvalidMutation)
            );
            assert_members_unchanged(&mut coordinator, before);
            assert_eq!(coordinator.next_id, next_id_before);
            assert_eq!(coordinator.uuids, uuids_before);
        }
    }
}

const POPULATION_FOOD: crate::villager_population_26_1_2::VillagerFoodItemIds =
    crate::villager_population_26_1_2::VillagerFoodItemIds {
        bread: 7,
        potato: 8,
        carrot: 9,
        beetroot: 10,
    };

fn parity_population_villager(position: Vec3, food: Option<(u32, i32)>) -> SpawnEntity {
    let mut villager = SpawnEntity::new(119, "minecraft:villager", position);
    let mut population = crate::villager_population_26_1_2::VillagerPopulationState::adult();
    if let Some((item_id, count)) = food {
        assert!(
            population
                .add_to_inventory(EntityItemStack::new(item_id, count), 64)
                .expect("seed villager inventory")
                .is_none()
        );
    }
    villager.retained.villager_population = Some(population);
    villager
}

fn parity_item(position: Vec3, item_id: u32, count: i32) -> SpawnEntity {
    let mut item = SpawnEntity::new(55, "minecraft:item", position);
    item.item_stack = Some(EntityItemStack::new(item_id, count));
    item
}

fn population_coordinator(
    journal: Option<Arc<Mutex<TestDecisionJournalState>>>,
) -> super::RegionalOwnerCoordinator {
    match journal {
        Some(journal) => super::RegionalOwnerCoordinator::from_store_with_journal(
            RegionalEntityStore::new(),
            2,
            Box::new(TestDecisionJournal(journal)),
        )
        .expect("journaled population coordinator"),
        None => super::RegionalOwnerCoordinator::from_store(RegionalEntityStore::new(), 2)
            .expect("population coordinator"),
    }
}

fn parity_courtship_commit(
    coordinator: &mut super::RegionalOwnerCoordinator,
    parents: [EntityId; 2],
) -> super::VillagerCourtshipCommit {
    let expected = parents.map(|id| {
        coordinator
            .snapshot(id)
            .expect("parent read")
            .expect("parent snapshot")
    });
    let mut next = expected.clone();
    for index in 0..2 {
        next[index]
            .retained
            .villager_population
            .as_mut()
            .expect("parent population")
            .start_pending_birth(expected[1 - index].uuid, 10, 0, false, POPULATION_FOOD)
            .expect("start reciprocal courtship");
        next[index].goal = GoalState::FollowTarget {
            target: expected[1 - index].id,
            speed: crate::villager_population_26_1_2::VILLAGER_COURTSHIP_SPEED,
        };
    }
    super::VillagerCourtshipCommit {
        parents: [
            (expected[0].clone(), next[0].clone()),
            (expected[1].clone(), next[1].clone()),
        ],
        current_tick: 10,
        food_items: POPULATION_FOOD,
        deterministic_seed: 0,
    }
}

fn pending_population_coordinator(
    journal: Option<Arc<Mutex<TestDecisionJournalState>>>,
) -> (super::RegionalOwnerCoordinator, [EntityId; 2]) {
    let mut coordinator = population_coordinator(journal);
    let parents = [
        coordinator
            .spawn(parity_population_villager(
                Vec3::new(0.5, 64.0, 0.5),
                Some((POPULATION_FOOD.bread, 3)),
            ))
            .expect("first parent"),
        coordinator
            .spawn(parity_population_villager(
                Vec3::new(1.5, 64.0, 0.5),
                Some((POPULATION_FOOD.potato, 12)),
            ))
            .expect("second parent"),
    ];
    let commit = parity_courtship_commit(&mut coordinator, parents);
    assert!(
        coordinator
            .commit_villager_courtship_if_current(commit)
            .expect("seed courtship")
    );
    (coordinator, parents)
}

fn parity_no_bed_commit(
    coordinator: &mut super::RegionalOwnerCoordinator,
    parents: [EntityId; 2],
) -> super::VillagerNoBedCommit {
    let expected = parents.map(|id| {
        coordinator
            .snapshot(id)
            .expect("parent read")
            .expect("pending parent")
    });
    let next = expected.clone().map(|mut snapshot| {
        snapshot
            .retained
            .villager_population
            .as_mut()
            .expect("pending population")
            .finish_courtship_without_child(285, POPULATION_FOOD)
            .expect("finish no-bed courtship");
        snapshot.goal = GoalState::Idle;
        snapshot
    });
    super::VillagerNoBedCommit {
        parents: [
            (expected[0].clone(), next[0].clone()),
            (expected[1].clone(), next[1].clone()),
        ],
        current_tick: 285,
        food_items: POPULATION_FOOD,
    }
}

fn parity_birth_commit(
    coordinator: &mut super::RegionalOwnerCoordinator,
    parents: [EntityId; 2],
    home: &str,
) -> super::VillagerBirthCommit {
    let expected = parents.map(|id| {
        coordinator
            .snapshot(id)
            .expect("parent read")
            .expect("pending parent")
    });
    let next = expected.clone().map(|mut snapshot| {
        snapshot
            .retained
            .villager_population
            .as_mut()
            .expect("pending population")
            .finish_successful_birth(285, POPULATION_FOOD)
            .expect("finish successful birth");
        snapshot.goal = GoalState::Idle;
        snapshot
    });
    let started_tick = expected[0]
        .retained
        .villager_population
        .as_ref()
        .and_then(|population| population.pending_birth.as_ref())
        .expect("pending birth")
        .started_tick;
    let mut child = SpawnEntity::new(119, "minecraft:villager", expected[0].position);
    child.uuid = Some(
        crate::villager_population_26_1_2::deterministic_villager_child_uuid(
            expected[0].uuid,
            expected[1].uuid,
            home,
            started_tick,
        ),
    );
    child.retained.villager_population =
        Some(crate::villager_population_26_1_2::VillagerPopulationState::baby(home.to_owned()));
    super::VillagerBirthCommit {
        parents: [
            (expected[0].clone(), next[0].clone()),
            (expected[1].clone(), next[1].clone()),
        ],
        child,
        current_tick: 285,
        food_items: POPULATION_FOOD,
    }
}

fn parity_snapshots(
    coordinator: &mut super::RegionalOwnerCoordinator,
    ids: impl IntoIterator<Item = EntityId>,
) -> Vec<(EntityId, EntitySnapshot)> {
    ids.into_iter()
        .map(|id| {
            (
                id,
                coordinator
                    .snapshot(id)
                    .expect("snapshot read")
                    .expect("snapshot exists"),
            )
        })
        .collect()
}

fn assert_parity_snapshots(
    coordinator: &mut super::RegionalOwnerCoordinator,
    expected: Vec<(EntityId, EntitySnapshot)>,
) {
    for (id, snapshot) in expected {
        assert_eq!(
            coordinator
                .snapshot(id)
                .expect("post-state read")
                .expect("entity remains"),
            snapshot
        );
    }
}

#[test]
fn villager_inventory_pickup_atomically_moves_exact_stack_and_keeps_remainder() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let mut coordinator = population_coordinator(Some(Arc::clone(&journal)));
    let mut villager_spawn = parity_population_villager(Vec3::new(0.5, 64.0, 0.5), None);
    let population = villager_spawn
        .retained
        .villager_population
        .as_mut()
        .expect("villager population");
    for item_id in 100..107 {
        assert!(
            population
                .add_to_inventory(EntityItemStack::new(item_id, 64), 64)
                .unwrap()
                .is_none()
        );
    }
    assert!(
        population
            .add_to_inventory(EntityItemStack::new(POPULATION_FOOD.carrot, 60), 64)
            .unwrap()
            .is_none()
    );
    let villager = coordinator.spawn(villager_spawn).expect("villager");
    let item = coordinator
        .spawn(parity_item(
            Vec3::new(0.75, 64.0, 0.5),
            POPULATION_FOOD.carrot,
            10,
        ))
        .expect("food item");
    let villager_expected = coordinator.snapshot(villager).unwrap().unwrap();
    let item_expected = coordinator.snapshot(item).unwrap().unwrap();
    let mut villager_next = villager_expected.clone();
    let remainder = villager_next
        .retained
        .villager_population
        .as_mut()
        .unwrap()
        .add_to_inventory(item_expected.item_stack.clone().unwrap(), 64)
        .unwrap();
    let mut item_next = item_expected.clone();
    item_next.item_stack = remainder;
    assert!(
        coordinator
            .commit_villager_inventory_pickup_if_current(super::VillagerInventoryPickupCommit {
                villager: (villager_expected, villager_next),
                item: (item_expected, Some(item_next)),
                item_max_stack_size: 64,
            },)
            .expect("pickup commit")
    );
    let population = coordinator
        .snapshot(villager)
        .unwrap()
        .unwrap()
        .retained
        .villager_population
        .unwrap();
    assert_eq!(population.inventory.count_item(POPULATION_FOOD.carrot), 64);
    assert_eq!(
        coordinator
            .snapshot(item)
            .unwrap()
            .unwrap()
            .item_stack
            .unwrap()
            .count,
        6
    );
    let decision = journal
        .lock()
        .unwrap()
        .commits
        .last()
        .cloned()
        .expect("pickup decision");
    assert_eq!(decision.upserts().len(), 2);
    assert!(decision.removed().is_empty());
}

#[test]
fn villager_inventory_pickup_returns_false_for_stale_or_out_of_reach_without_mutation() {
    for stale in [false, true] {
        let mut coordinator = population_coordinator(None);
        let villager = coordinator
            .spawn(parity_population_villager(Vec3::new(0.5, 64.0, 0.5), None))
            .unwrap();
        let item_position = if stale {
            Vec3::new(0.75, 64.0, 0.5)
        } else {
            Vec3::new(4.0, 64.0, 0.5)
        };
        let item = coordinator
            .spawn(parity_item(item_position, POPULATION_FOOD.bread, 3))
            .unwrap();
        let villager_expected = coordinator.snapshot(villager).unwrap().unwrap();
        let item_expected = coordinator.snapshot(item).unwrap().unwrap();
        let mut villager_next = villager_expected.clone();
        let remainder = villager_next
            .retained
            .villager_population
            .as_mut()
            .unwrap()
            .add_to_inventory(item_expected.item_stack.clone().unwrap(), 64)
            .unwrap();
        let item_next = remainder.map(|stack| {
            let mut next = item_expected.clone();
            next.item_stack = Some(stack);
            next
        });
        let commit = super::VillagerInventoryPickupCommit {
            villager: (villager_expected, villager_next),
            item: (item_expected, item_next),
            item_max_stack_size: 64,
        };
        if stale {
            coordinator
                .set_velocities([(item, Vec3::new(0.1, 0.0, 0.0))])
                .unwrap();
            let before = parity_snapshots(&mut coordinator, [villager, item]);
            assert!(
                !coordinator
                    .commit_villager_inventory_pickup_if_current(commit)
                    .unwrap()
            );
            assert_parity_snapshots(&mut coordinator, before);
        } else {
            let before = parity_snapshots(&mut coordinator, [villager, item]);
            assert!(
                !coordinator
                    .commit_villager_inventory_pickup_if_current(commit)
                    .unwrap()
            );
            assert_parity_snapshots(&mut coordinator, before);
        }
    }
}

#[test]
fn villager_inventory_pickup_accepts_targeted_shared_food_within_search_radius() {
    let mut coordinator = population_coordinator(None);
    let villager = coordinator
        .spawn(parity_population_villager(Vec3::new(0.5, 64.0, 0.5), None))
        .unwrap();
    let mut shared_food = parity_item(Vec3::new(2.25, 64.0, 0.5), POPULATION_FOOD.carrot, 3);
    shared_food.retained.villager_food_recipient = Some(villager);
    let item = coordinator.spawn(shared_food).unwrap();
    let villager_expected = coordinator.snapshot(villager).unwrap().unwrap();
    let item_expected = coordinator.snapshot(item).unwrap().unwrap();
    let mut villager_next = villager_expected.clone();
    let remainder = villager_next
        .retained
        .villager_population
        .as_mut()
        .unwrap()
        .add_to_inventory(item_expected.item_stack.clone().unwrap(), 64)
        .unwrap();

    assert!(remainder.is_none());
    assert!(
        coordinator
            .commit_villager_inventory_pickup_if_current(super::VillagerInventoryPickupCommit {
                villager: (villager_expected, villager_next),
                item: (item_expected, None),
                item_max_stack_size: 64,
            },)
            .unwrap()
    );
    assert!(coordinator.snapshot(item).unwrap().is_none());
}

#[test]
fn villager_courtship_sets_reciprocal_target_without_consuming_inventory_food() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let mut coordinator = population_coordinator(Some(Arc::clone(&journal)));
    let parents = [
        coordinator
            .spawn(parity_population_villager(
                Vec3::new(0.5, 64.0, 0.5),
                Some((POPULATION_FOOD.bread, 3)),
            ))
            .unwrap(),
        coordinator
            .spawn(parity_population_villager(
                Vec3::new(1.5, 64.0, 0.5),
                Some((POPULATION_FOOD.carrot, 12)),
            ))
            .unwrap(),
    ];
    let commit = parity_courtship_commit(&mut coordinator, parents);
    assert!(
        coordinator
            .commit_villager_courtship_if_current(commit)
            .unwrap()
    );
    for (index, parent) in parents.into_iter().enumerate() {
        let snapshot = coordinator.snapshot(parent).unwrap().unwrap();
        assert_eq!(
            snapshot.goal,
            GoalState::FollowTarget {
                target: parents[1 - index],
                speed: crate::villager_population_26_1_2::VILLAGER_COURTSHIP_SPEED,
            }
        );
        let population = snapshot.retained.villager_population.unwrap();
        assert_eq!(population.food_level, 0);
        assert_eq!(population.inventory.food_points(POPULATION_FOOD), 12);
        let pending = population.pending_birth.unwrap();
        assert_eq!(pending.started_tick, 10);
        assert_eq!(pending.ready_tick, 285);
    }
    assert_eq!(
        journal
            .lock()
            .unwrap()
            .commits
            .last()
            .unwrap()
            .upserts()
            .len(),
        2
    );
}

#[test]
fn villager_no_bed_digests_food_without_child_or_parent_cooldown() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let (mut coordinator, parents) = pending_population_coordinator(Some(Arc::clone(&journal)));
    let commit = parity_no_bed_commit(&mut coordinator, parents);
    assert!(
        coordinator
            .commit_villager_no_bed_if_current(commit)
            .unwrap()
    );
    for parent in parents {
        let population = coordinator
            .snapshot(parent)
            .unwrap()
            .unwrap()
            .retained
            .villager_population
            .unwrap();
        assert_eq!(population.food_level, 0);
        assert_eq!(population.inventory.food_points(POPULATION_FOOD), 0);
        assert_eq!(population.age_ticks, 0);
        assert!(population.pending_birth.is_none());
    }
    assert_eq!(
        journal
            .lock()
            .unwrap()
            .commits
            .last()
            .unwrap()
            .upserts()
            .len(),
        2
    );
}

#[test]
fn villager_no_bed_journal_failure_rolls_back_both_parents() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let (mut coordinator, parents) = pending_population_coordinator(Some(Arc::clone(&journal)));
    let commit = parity_no_bed_commit(&mut coordinator, parents);
    let before = parity_snapshots(&mut coordinator, parents);
    journal.lock().unwrap().fail_record = true;
    assert_eq!(
        coordinator.commit_villager_no_bed_if_current(commit),
        Err(super::RegionOwnerLaneError::Journal)
    );
    assert_parity_snapshots(&mut coordinator, before);
}

#[test]
fn villager_birth_digests_food_sets_cooldowns_and_is_replay_safe() {
    let journal = Arc::new(Mutex::new(TestDecisionJournalState::default()));
    let (mut coordinator, parents) = pending_population_coordinator(Some(Arc::clone(&journal)));
    let commit = parity_birth_commit(&mut coordinator, parents, "home:child");
    let replay = commit.clone();
    let next_id = coordinator.next_id + 1;
    let child = coordinator
        .commit_villager_birth_if_current(commit)
        .unwrap()
        .expect("child commit");
    assert_eq!(child.id, EntityId(next_id));
    assert_eq!(
        child
            .retained
            .villager_population
            .as_ref()
            .unwrap()
            .claimed_home
            .as_deref(),
        Some("home:child")
    );
    for parent in parents {
        let population = coordinator
            .snapshot(parent)
            .unwrap()
            .unwrap()
            .retained
            .villager_population
            .unwrap();
        assert_eq!(population.food_level, 0);
        assert_eq!(population.inventory.food_points(POPULATION_FOOD), 0);
        assert_eq!(
            population.age_ticks,
            crate::villager_population_26_1_2::VILLAGER_PARENT_COOLDOWN_TICKS
        );
        assert!(population.pending_birth.is_none());
    }
    let next_id_after = coordinator.next_id;
    assert!(
        coordinator
            .commit_villager_birth_if_current(replay)
            .unwrap()
            .is_none()
    );
    assert_eq!(coordinator.next_id, next_id_after);
    assert!(
        journal
            .lock()
            .unwrap()
            .commits
            .last()
            .unwrap()
            .upserts()
            .iter()
            .any(|snapshot| snapshot.id == child.id)
    );
}

#[test]
fn villager_birth_rejects_stale_or_wrong_child_identity_without_allocation() {
    for wrong_uuid in [false, true] {
        let (mut coordinator, parents) = pending_population_coordinator(None);
        let mut commit = parity_birth_commit(&mut coordinator, parents, "home:child");
        let next_id_before = coordinator.next_id;
        if wrong_uuid {
            commit.child.uuid = Some(Uuid::from_u128(999));
            assert_eq!(
                coordinator.commit_villager_birth_if_current(commit),
                Err(super::RegionOwnerLaneError::InvalidMutation)
            );
        } else {
            coordinator
                .set_velocities([(parents[0], Vec3::new(0.1, 0.0, 0.0))])
                .unwrap();
            assert!(
                coordinator
                    .commit_villager_birth_if_current(commit)
                    .unwrap()
                    .is_none()
            );
        }
        assert_eq!(coordinator.next_id, next_id_before);
    }
}
