//! Retained interest and delivery debt; ordinary publishes visit changed rows only.
use super::*;
use std::{cmp::Reverse, collections::BinaryHeap};

type Order = (i32, i32, i32);

#[derive(Debug, Default)]
pub(super) struct EntityInterest {
    previous: Option<mc_core::boundary::EntityViews>,
    position: Option<([u64; 2], i32)>,
    held: HashSet<(i32, i32)>,
    ranges: HashSet<i32>,
    visible: HashMap<EntityId, Order>,
    spawns: HashSet<EntityId>,
    removals: HashSet<EntityId>,
    bosses: HashSet<EntityId>,
    dirty: HashSet<EntityId>,
    ready: HashSet<EntityId>,
    scheduled: HashMap<EntityId, u64>,
    due: BinaryHeap<Reverse<(u64, i32)>>,
    order: Vec<Order>,
    resume: Option<Order>,
    seeded: bool,
    seed: Vec<EntityId>,
}

impl EntityInterest {
    pub(super) fn start(&self, work: &[EntityId], snapshot: &WorldSnapshot) -> usize {
        self.resume
            .and_then(|resume| {
                work.iter().position(|id| {
                    self.visible
                        .get(id)
                        .copied()
                        .or_else(|| snapshot.entities.get(*id).map(order))
                        .is_some_and(|key| key >= resume)
                })
            })
            .unwrap_or(0)
    }

    pub(super) fn blocked(&mut self, entity: &EntityView) {
        self.resume = Some(order(entity));
    }

    pub(super) fn previous(&self) -> Option<&mc_core::boundary::EntityViews> {
        self.previous.as_ref()
    }

    pub(super) fn prepare(
        &mut self,
        snapshot: &WorldSnapshot,
        viewer: &mut ViewerState,
        distance: i32,
        index: &TickPlanIndex,
        scratch: &mut PlanScratch,
        credit: usize,
    ) {
        let position = (
            [viewer.position[0].to_bits(), viewer.position[2].to_bits()],
            distance.max(0),
        );
        let revision = self
            .previous
            .as_ref()
            .map(mc_core::boundary::EntityViews::revision);
        let same = revision == Some(snapshot.entities.revision());
        let contiguous =
            same || revision.is_some_and(|revision| index.entity_base == Some(revision));
        let held_same = self.held.len() == viewer.loaded_chunks.len()
            && self
                .held
                .iter()
                .all(|pos| viewer.loaded_chunks.contains_key(pos));
        let window_same = self.position.is_some_and(|(old, radius)| {
            radius == position.1
                && entity_chunk([f64::from_bits(old[0]), 0.0, f64::from_bits(old[1])])
                    == entity_chunk(viewer.position)
        });
        if !contiguous || !window_same || !held_same {
            entity_candidates::collect(snapshot, viewer, distance, scratch, credit);
            self.ranges.clear();
            self.ranges.extend(scratch.tracking_ranges.iter().copied());
            self.visible.clear();
            self.spawns.clear();
            self.removals.clear();
            self.bosses.clear();
            self.dirty.clear();
            self.ready.clear();
            self.due.clear();
            self.scheduled.clear();
            // A gap/window repair already visits all rows below. Retain the
            // canonical ids cheaply, and build the incremental lookup only
            // if the next publication actually has a usable base. Rebuilt
            // snapshots must not pay for an index they cannot reuse.
            self.seeded = false;
            self.seed.clone_from(&scratch.visible);
            self.bosses.extend(scratch.bosses.iter().copied());
            for (&id, tracked) in &viewer.tracked_entities {
                if id != viewer.entity_id && tracked.seen != viewer.plan_stamp {
                    self.removals.insert(id);
                }
            }
            self.held.clear();
            self.held.extend(viewer.loaded_chunks.keys().copied());
        } else {
            if !self.seeded {
                let previous = self.previous.as_ref().expect("interest baseline");
                for &id in &self.seed {
                    let entity = previous.get(id).expect("retained visible entity");
                    self.visible.insert(id, order(entity));
                    if let Some(tracked) = viewer.tracked_entities.get(&id)
                        && tracked.type_id == entity.type_id
                    {
                        if !Arc::ptr_eq(&tracked.appearance, &entity.appearance)
                            || entity_movement(entity, tracked.baseline).is_some()
                            || tracked.head_yaw != pack_degrees(entity.head_yaw)
                        {
                            self.dirty.insert(id);
                        }
                    } else if id != viewer.entity_id {
                        self.spawns.insert(id);
                    }
                }
                self.seeded = true;
            }
            scratch.visible.clear();
            if !same {
                index.entity_changes_for(&viewer.loaded_chunks, |delta| {
                    self.delta(snapshot, viewer, distance, delta);
                });
                // The player's own image need not belong to a held chunk.
                if !viewer
                    .loaded_chunks
                    .contains_key(&entity_chunk(viewer.position))
                    && snapshot
                        .entities
                        .changed()
                        .binary_search(&viewer.entity_id)
                        .is_ok()
                {
                    self.changed(snapshot, viewer, distance, viewer.entity_id);
                }
            }
            if self.position != Some(position) {
                let (old, _) = self.position.expect("retained interest window");
                self.enter_leave(snapshot, viewer, distance, old, scratch);
            }
            scratch.visible.sort_unstable_by_key(|id| id.0);
            scratch.visible.dedup();
            for &id in &scratch.visible {
                self.changed(snapshot, viewer, distance, id);
            }
            self.order.clear();
            while let Some(&Reverse((tick, id))) = self.due.peek() {
                if tick > snapshot.tick {
                    break;
                }
                self.due.pop();
                let id = EntityId(id);
                if self.scheduled.get(&id) != Some(&tick) {
                    continue;
                }
                self.scheduled.remove(&id);
                if self.visible.contains_key(&id) && viewer.tracked_entities.contains_key(&id) {
                    self.ready.insert(id);
                    self.dirty.insert(id);
                }
            }
            self.order.extend(
                self.dirty
                    .iter()
                    .filter_map(|id| self.visible.get(id).copied()),
            );
            self.order.sort_unstable();
            scratch.visible.clear();
            scratch
                .visible
                .extend(self.order.iter().map(|key| EntityId(key.2)));
            scratch.spawns.clear();
            if credit > 0 {
                self.order.clear();
                self.order
                    .extend(self.spawns.iter().map(|id| self.visible[id]));
                self.order.sort_unstable();
                for key in &self.order {
                    let id = EntityId(key.2);
                    let entity = snapshot.entities.get(id).expect("pending spawn");
                    let distance = entity
                        .position
                        .iter()
                        .zip(viewer.position)
                        .map(|(a, b)| (a - b).powi(2))
                        .sum();
                    scratch.spawns.push((id, distance));
                }
            }
            scratch.bosses.clear();
            scratch.bosses.extend(self.bosses.iter().copied());
            scratch.bosses.sort_unstable_by_key(|id| self.visible[id]);
        }
        scratch.removed.clear();
        scratch.removed.extend(self.removals.iter().map(|id| id.0));
        scratch.removed.sort_unstable();
        self.position = Some(position);
        self.previous = Some(snapshot.entities.clone());
    }

    fn delta(
        &mut self,
        snapshot: &WorldSnapshot,
        viewer: &ViewerState,
        distance: i32,
        delta: tick_index::EntityDelta,
    ) {
        let id = delta.id;
        if id == viewer.entity_id {
            self.changed(snapshot, viewer, distance, id);
            return;
        }
        let was_visible = self.visible.contains_key(&id);
        let range = f64::from(delta.range.min(distance.max(0))) * 16.0;
        let interior = delta.membership_same
            && was_visible
            && !self.spawns.contains(&id)
            && delta.chunk.is_some_and(|(x, z)| {
                let min = [f64::from(x) * 16.0, f64::from(z) * 16.0];
                min[0] >= viewer.position[0] - range
                    && min[0] + 16.0 <= viewer.position[0] + range
                    && min[1] >= viewer.position[2] - range
                    && min[1] + 16.0 <= viewer.position[2] + range
            });
        if !interior {
            self.changed(snapshot, viewer, distance, id);
        } else if delta.appearance_changed || delta.motion_changed {
            // Keep changed movement in canonical credit order even before
            // its due tick. Only completed rows may arm a deadline: arming
            // rows behind a blocked cursor would invent delivery debt.
            self.dirty.insert(id);
        }
    }

    fn enter_leave(
        &self,
        snapshot: &WorldSnapshot,
        viewer: &ViewerState,
        distance: i32,
        old: [u64; 2],
        scratch: &mut PlanScratch,
    ) {
        let old = [f64::from_bits(old[0]), f64::from_bits(old[1])];
        let new = [viewer.position[0], viewer.position[2]];
        scratch.touched.clear();
        for &range in &self.ranges {
            let r = f64::from(range) * 16.0;
            if old[0] != new[0] {
                for edge in [-r, r] {
                    strip_chunks(
                        [old[0].min(new[0]) + edge, old[1].min(new[1]) - r],
                        [old[0].max(new[0]) + edge, old[1].max(new[1]) + r],
                        &mut scratch.touched,
                    );
                }
            }
            if old[1] != new[1] {
                for edge in [-r, r] {
                    strip_chunks(
                        [old[0].min(new[0]) - r, old[1].min(new[1]) + edge],
                        [old[0].max(new[0]) + r, old[1].max(new[1]) + edge],
                        &mut scratch.touched,
                    );
                }
            }
        }
        for &(x, z) in &scratch.touched {
            if !viewer.loaded_chunks.contains_key(&(x, z)) {
                continue;
            }
            for entity in snapshot
                .entities
                .in_chunk(mc_core::boundary::ChunkPos { x, z })
            {
                if entity_trackable(entity, viewer, distance)
                    != self.visible.contains_key(&entity.id)
                {
                    scratch.visible.push(entity.id);
                }
            }
        }
    }

    fn changed(
        &mut self,
        snapshot: &WorldSnapshot,
        viewer: &ViewerState,
        distance: i32,
        id: EntityId,
    ) {
        let entity = snapshot.entities.get(id).filter(|entity| {
            let chunk = entity_chunk(entity.position);
            if viewer.loaded_chunks.contains_key(&chunk)
                && let Some(range) = tracking_range(entity.type_id, distance)
            {
                self.ranges.insert(range);
            }
            let center = entity_chunk(viewer.position);
            (chunk.0 - center.0).abs() <= distance.max(0)
                && (chunk.1 - center.1).abs() <= distance.max(0)
                && (id == viewer.entity_id || viewer.loaded_chunks.contains_key(&chunk))
                && entity_trackable(entity, viewer, distance)
        });
        if let Some(entity) = entity {
            self.visible.insert(id, order(entity));
            if entity_type_flags::is_boss(entity.type_id) {
                self.bosses.insert(id);
            } else {
                self.bosses.remove(&id);
            }
            if viewer
                .tracked_entities
                .get(&id)
                .is_some_and(|sent| sent.type_id == entity.type_id)
            {
                self.removals.remove(&id);
                self.spawns.remove(&id);
                self.dirty.insert(id);
            } else if id != viewer.entity_id {
                if viewer.tracked_entities.contains_key(&id) {
                    self.removals.insert(id);
                }
                self.spawns.insert(id);
                self.dirty.remove(&id);
            }
        } else {
            self.visible.remove(&id);
            self.spawns.remove(&id);
            self.bosses.remove(&id);
            self.clean(id);
            if id != viewer.entity_id && viewer.tracked_entities.contains_key(&id) {
                self.removals.insert(id);
            }
        }
    }

    fn clean(&mut self, id: EntityId) {
        self.dirty.remove(&id);
        self.ready.remove(&id);
        self.scheduled.remove(&id);
    }

    pub(super) fn removed(&mut self, id: EntityId) {
        self.removals.remove(&id);
        self.clean(id);
    }

    pub(super) fn spawned(&mut self, id: EntityId) {
        self.spawns.remove(&id);
        self.clean(id);
    }

    pub(super) fn ready(&self, id: EntityId) -> bool {
        self.ready.contains(&id)
    }

    pub(super) fn completed(
        &mut self,
        entity: &EntityView,
        tracked: &TrackedEntity,
        tick: u64,
        due: bool,
    ) {
        if !self.seeded {
            return;
        }
        self.dirty.remove(&entity.id);
        self.ready.remove(&entity.id);
        if !due
            && (entity_movement(entity, tracked.baseline).is_some()
                || tracked.head_yaw != pack_degrees(entity.head_yaw))
        {
            let interval =
                mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(entity.type_id)
                    .map_or(1, |contract| u64::from(contract.update_interval).max(1));
            let phase =
                (tick % interval + entity.id.0.rem_euclid(interval as i32) as u64) % interval;
            let next = tick.saturating_add(interval - phase);
            if self.scheduled.insert(entity.id, next) != Some(next) {
                self.due.push(Reverse((next, entity.id.0)));
            }
        } else {
            self.scheduled.remove(&entity.id);
        }
    }
}

fn order(entity: &EntityView) -> Order {
    let chunk = entity_chunk(entity.position);
    (chunk.0, chunk.1, entity.id.0)
}

pub(super) fn tracking_range(type_id: u32, distance: i32) -> Option<i32> {
    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
        .map(|contract| (contract.tracking_range as i32).min(distance.max(0)))
}

fn strip_chunks(min: [f64; 2], max: [f64; 2], chunks: &mut HashSet<(i32, i32)>) {
    let min = entity_chunk([min[0], 0.0, min[1]]);
    let max = entity_chunk([max[0], 0.0, max[1]]);
    for x in min.0..=max.0 {
        for z in min.1..=max.1 {
            chunks.insert((x, z));
        }
    }
}
