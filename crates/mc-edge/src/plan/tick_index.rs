//! One change index per dimension/publish, shared by every viewer.
use super::*;

#[derive(Default)]
struct SubjectEvents {
    chunk: Option<(i32, i32)>,
    events: Vec<usize>,
}

/// Publish-wide facts: neither membership lookup nor type inspection needs
/// repeating for every viewer tracking the same moving mob.
#[derive(Clone, Copy)]
pub(super) struct EntityDelta {
    pub(super) id: EntityId,
    pub(super) chunk: Option<(i32, i32)>,
    pub(super) membership_same: bool,
    pub(super) appearance_changed: bool,
    pub(super) motion_changed: bool,
    pub(super) range: i32,
}

#[derive(Default)]
pub(crate) struct TickPlanIndex {
    pub(super) blocks: HashMap<(i32, i32), Vec<usize>>,
    pub(super) explosions: HashMap<(i32, i32), Vec<usize>>,
    pub(super) events: HashMap<(i32, i32), Vec<usize>>,
    pub(super) entity_base: Option<u64>,
    /// Anti-xray: hideable ores a block change this tick just exposed, keyed by
    /// the ore's own chunk (the change may be in a neighbouring chunk).
    pub(super) reveals: HashMap<(i32, i32), Vec<mc_world::chunk::BlockPos>>,
    entity_chunks: HashMap<(i32, i32), Vec<EntityDelta>>,
    pub(super) moves: super::shared_moves::SharedMoves,
    // Events can refer to an entity already removed from the snapshot, or to
    // a known entity that crossed out of the viewer square before its event.
    event_subjects: HashMap<EntityId, SubjectEvents>,
}

impl TickPlanIndex {
    /// Add the anti-xray reveals of this tick's block changes (computed once
    /// per publish, shared by every viewer).
    pub(crate) fn with_reveals(
        mut self,
        output: &TickOutput,
        policy: Option<&crate::anti_xray::AntiXray>,
    ) -> Self {
        let Some(policy) = policy.filter(|policy| policy.enabled(output.snapshot.dimension)) else {
            return self;
        };
        let mut revealed = Vec::new();
        for change in &output.changes.blocks {
            policy.reveal_around(
                output.snapshot.dimension,
                &output.snapshot.chunks,
                change.position,
                &mut revealed,
            );
        }
        revealed.sort_unstable_by_key(|pos| (pos.x, pos.y, pos.z));
        revealed.dedup();
        for pos in revealed {
            self.reveals
                .entry((pos.x.div_euclid(16), pos.z.div_euclid(16)))
                .or_default()
                .push(pos);
        }
        self
    }

    pub(crate) fn new(output: &TickOutput) -> Self {
        let mut index = Self::default();
        for (i, change) in output.changes.blocks.iter().enumerate() {
            let pos = (
                change.position.x.div_euclid(16),
                change.position.z.div_euclid(16),
            );
            index.blocks.entry(pos).or_default().push(i);
        }
        for (i, explosion) in output.changes.explosions.iter().enumerate() {
            index
                .explosions
                .entry(entity_chunk(explosion.center))
                .or_default()
                .push(i);
        }
        for (i, event) in output.changes.entity_events.iter().enumerate() {
            index
                .event_subjects
                .entry(event_subject(*event))
                .or_default()
                .events
                .push(i);
        }
        if !index.event_subjects.is_empty() {
            for (id, subject) in &mut index.event_subjects {
                subject.chunk = output
                    .snapshot
                    .entities
                    .get(*id)
                    .map(|entity| entity_chunk(entity.position));
            }
            for subject in index.event_subjects.values() {
                if let Some(pos) = subject.chunk {
                    index.events.entry(pos).or_default().extend(&subject.events);
                }
            }
        }
        index
    }

    pub(crate) fn with_previous(
        output: &TickOutput,
        previous: Option<&mc_core::boundary::EntityViews>,
    ) -> Self {
        let mut index = Self::new(output);
        let entities = &output.snapshot.entities;
        let Some(previous) = previous else {
            return index;
        };
        if previous.revision() == entities.revision() {
            index.entity_base = Some(previous.revision());
        } else if previous.revision() == entities.base_revision() {
            index.entity_base = Some(previous.revision());
            for &id in entities.changed() {
                let old = previous.get(id);
                let new = entities.get(id);
                let old_chunk = old.map(|entity| entity_chunk(entity.position));
                let chunk = new.map(|entity| entity_chunk(entity.position));
                let contract = new.and_then(|entity| {
                    mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                        entity.type_id,
                    )
                });
                let delta = EntityDelta {
                    id,
                    chunk,
                    membership_same: old
                        .zip(new)
                        .is_some_and(|(old, new)| old.type_id == new.type_id && old_chunk == chunk),
                    appearance_changed: old
                        .zip(new)
                        .is_none_or(|(old, new)| !Arc::ptr_eq(&old.appearance, &new.appearance)),
                    motion_changed: old.zip(new).is_none_or(|(old, new)| {
                        old.position != new.position
                            || old.yaw != new.yaw
                            || old.pitch != new.pitch
                            || old.head_yaw != new.head_yaw
                    }),
                    range: contract.map_or(0, |contract| i32::from(contract.tracking_range)),
                };
                if let Some(chunk) = old_chunk {
                    index.entity_chunks.entry(chunk).or_default().push(delta);
                }
                if let Some(chunk) = chunk.filter(|chunk| Some(*chunk) != old_chunk) {
                    index.entity_chunks.entry(chunk).or_default().push(delta);
                }
            }
        }
        index
    }

    pub(super) fn entity_changes_for<V>(
        &self,
        chunks: &HashMap<(i32, i32), V>,
        mut changed: impl FnMut(EntityDelta),
    ) {
        for chunk in chunks.keys() {
            if let Some(deltas) = self.entity_chunks.get(chunk) {
                for &delta in deltas {
                    // Cross-chunk rows occur in both buckets. Prefer their
                    // current held bucket; the old one still routes departures.
                    if delta.chunk != Some(*chunk)
                        && delta.chunk.is_some_and(|pos| chunks.contains_key(&pos))
                    {
                        continue;
                    }
                    changed(delta);
                }
            }
        }
    }

    pub(super) fn explosions_near(&self, position: [f64; 3], candidates: &mut Vec<usize>) {
        candidates.clear();
        if self.explosions.is_empty() {
            return;
        }
        let center = entity_chunk(position);
        for x in center.0 - 4..=center.0 + 4 {
            for z in center.1 - 4..=center.1 + 4 {
                if let Some(events) = self.explosions.get(&(x, z)) {
                    candidates.extend(events);
                }
            }
        }
        candidates.sort_unstable();
    }

    pub(super) fn events_for(
        &self,
        want: &HashSet<(i32, i32)>,
        subjects: impl Iterator<Item = EntityId>,
        candidates: &mut Vec<usize>,
    ) {
        candidates.clear();
        if self.event_subjects.is_empty() {
            return;
        }
        // Most events route via the held interest square. The subject index
        // handles deaths/pickups absent from the snapshot and events preceding
        // removal after a known entity moved out of that square.
        for pos in want {
            if let Some(events) = self.events.get(pos) {
                candidates.extend(events);
            }
        }
        for subject in subjects {
            if let Some(subject) = self.event_subjects.get(&subject)
                && subject.chunk.is_none_or(|pos| !want.contains(&pos))
            {
                candidates.extend(&subject.events);
            }
        }
        candidates.sort_unstable();
        candidates.dedup();
    }
}

pub(super) fn entity_chunk(position: [f64; 3]) -> (i32, i32) {
    (
        (position[0] / 16.0).floor() as i32,
        (position[2] / 16.0).floor() as i32,
    )
}

pub(super) fn event_subject(event: EntityEvent) -> EntityId {
    match event {
        EntityEvent::Swing { entity, .. }
        | EntityEvent::Hurt { entity }
        | EntityEvent::Damage { entity, .. }
        | EntityEvent::ShieldBlock { entity, .. }
        | EntityEvent::GoldenDandelion { entity, .. }
        | EntityEvent::Bottle { entity, .. }
        | EntityEvent::SweepAttack { entity, .. }
        | EntityEvent::Death { entity }
        | EntityEvent::Status { entity, .. } => entity,
        EntityEvent::Pickup { item, .. } => item,
    }
}
