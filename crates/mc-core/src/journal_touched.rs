//! Entity journaling over the touched set: O(touched) per batch, not O(entities).
//!
//! Owner custody is `EntityStore::{row_mut, body_mut, parts_mut}` (and
//! spawn/transfer-in): retained mutable access lists the entity for the next
//! batch. The journal compares and emits only listed rows against
//! their last journaled image; equality stays the final arbiter, so a
//! conservative mark costs one comparison, never correctness. No hashing and
//! no per-field setters.
//!
//! Invariants this relies on:
//! - Every live row is in `journaled_villagers`/`journaled_entity_state` (as
//!   far as it needs one) or in the touched set: rows enter the store through
//!   spawn/transfer-in, which list them.
//! - Deterministic clock projections ([`advance_clocks`]) are lazy: an image
//!   is projected from its own anchor tick to the batch tick only when its row
//!   is compared. An untouched villager row equals that projection because its
//!   clock owner advances the live row exactly as the projection does (brain,
//!   or the non-marking clock projection of skipped brains). Generic live
//!   lifecycle clocks also advance without marks; resets/growth mark the row.
//!   Dead clock-sensitive rows still need rechecks because their clocks stop.
//! - Chunk clock admission is not a row mutation. Each affected chunk rebases
//!   its entity anchors before its boundary image is drained; unrelated chunk
//!   residency and activity cannot force a population-wide comparison.

use std::sync::Arc;

use crate::boundary::{EntityEvent, EntityId, TickOutput};
use crate::entities::{EntityKind, EntitySlot};
use crate::journal::{JournalEntry, entity_id_int};
use crate::tick::TickState;

/// Persisted pose at the last full image; batch time is not a change key.
#[derive(Debug, PartialEq)]
struct MotionPose {
    position: [f64; 3],
    velocity: [f64; 3],
    rotation: [f32; 2],
}

/// Journal-side entity caches that live next to the baseline images.
#[derive(Debug, Default)]
pub(crate) struct JournalScan {
    /// Motion-capable bodies and their last emitted pose. Retained state uses
    /// the ordinary projected-image comparison below, even for a touched body.
    motion: std::collections::HashMap<EntityId, Option<MotionPose>>,
    /// Dead ids whose stopped live clocks differ from the replay projection
    /// and must be compared again next batch even if untouched.
    clock_recheck: Vec<EntityId>,
    /// The first batch seeds all baselines; later boundaries use chunk indexes.
    initialized: bool,
}

/// Touched rows of the latest batch (bench receipts).
#[cfg(test)]
pub(crate) static LAST_TOUCHED: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);

pub(crate) fn entity_images(
    state: &mut TickState,
    output: &TickOutput,
    entries: &mut Vec<JournalEntry>,
) {
    let mut touched = state.entities.take_touched();
    #[cfg(test)]
    LAST_TOUCHED.store(touched.len(), std::sync::atomic::Ordering::Relaxed);
    for id in state.entities.take_removed() {
        // A same-id row that is live again keeps its (re)seeded baseline.
        if state.entities.slot_of_any(id).is_some() {
            continue;
        }
        state.journaled_villagers.remove(&id);
        state.journaled_entity_state.remove(&id);
        state.journal_scan.motion.remove(&id);
    }
    if state.journaled_entity_state.is_empty() {
        state.journaled_entity_state.reserve(state.entities.len());
    }

    // Entity post-images: only entities that changed this tick. Spawns and
    // appearance changes (e.g. a partly picked-up item stack) carry the full
    // record; hurt-only updates carry health. Replay stays idempotent
    // (absolute post-images); unchanged entities are covered by the
    // checkpoint and earlier batches. Full snapshots every tick do not
    // scale (50k entities ~= tens of MB per batch and stall the tick thread).
    let mut full: Vec<EntityId> = output
        .changes
        .entities_spawned
        .iter()
        .chain(output.changes.entities_appearance_changed.iter())
        .copied()
        .collect();
    if !std::mem::replace(&mut state.journal_scan.initialized, true) {
        touched.clear();
        touched.extend(state.entities.slots().filter_map(|slot| {
            let body = state.entities.body(slot);
            (!matches!(body.kind, EntityKind::Player { .. })).then_some(body.id)
        }));
    }
    let owner_count = touched.len();
    for chunk in state.world.simulation_clock_changes() {
        touched.extend(
            state
                .entities
                .villager_chunks()
                .get(&chunk)
                .into_iter()
                .flatten()
                .copied(),
        );
        touched.extend(
            state
                .entities
                .brain_sensor_chunks()
                .get(&chunk)
                .into_iter()
                .flatten()
                .copied(),
        );
    }
    if touched.len() != owner_count {
        touched.sort_unstable();
        touched.dedup();
    }
    for id in &touched {
        let Some(slot) = state.entities.slot_of_any(*id) else {
            continue;
        };
        let row = state.entities.row(slot);
        if row.villager.is_none() {
            state.journaled_villagers.remove(id);
        } else {
            villager_image(state, slot, output.tick, &mut full);
        }
        // Falling bodies, TNT and vehicles couple retained custody/fuses to
        // motion. Every update travels with its authoritative position and
        // velocity.
        let row = state.entities.row(slot);
        // Other bodies carry a full image only when crossing chunk-clock
        // authority; intra-chunk movement stays checkpoint-only. The move
        // itself marks the row touched (`EntityStore::set_pos`).
        let chunk = chunk_position(state, slot);
        let crossed = state
            .journaled_entity_state
            .get(id)
            .is_some_and(|(_, old_chunk, _)| *old_chunk != chunk);
        if crossed {
            full.push(*id);
        }
        let tracks_motion = row.falling.is_some()
            || row.tnt.is_some()
            || row.entity_life.vehicle.is_some()
            || crate::end_fight::is_dragon(state, slot);
        if tracks_motion {
            state.journal_scan.motion.entry(*id).or_default();
        } else {
            state.journal_scan.motion.remove(id);
        }
    }
    full.extend(
        state
            .journal_scan
            .motion
            .iter()
            .filter_map(|(&id, previous)| {
                let slot = state.entities.slot_of_any(id)?;
                let [yaw, pitch, _] = state.entities.rot(slot);
                let pose = MotionPose {
                    position: state.entities.pos(slot),
                    velocity: state.entities.vel(slot),
                    rotation: [yaw, pitch],
                };
                (previous.as_ref() != Some(&pose)).then_some(id)
            }),
    );
    full.sort_unstable();
    full.dedup();
    let mut hurt: Vec<EntityId> = output
        .changes
        .entity_events
        .iter()
        .filter_map(|event| match *event {
            EntityEvent::Hurt { entity }
            | EntityEvent::Damage { entity, .. }
            | EntityEvent::Death { entity } => Some(entity),
            _ => None,
        })
        .filter(|entity| full.binary_search(entity).is_err())
        .collect();
    hurt.sort_unstable();
    hurt.dedup();
    for id in &hurt {
        let Some(slot) = state.entities.slot_of_any(*id) else {
            continue;
        };
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if matches!(body.kind, EntityKind::Player { .. }) {
            continue;
        }
        entries.push(JournalEntry::EntityVitals {
            id: entity_id_int(body.id),
            health: row.health,
            alive: body.alive,
            tick: output.tick,
        });
    }
    for id in &full {
        let Some(slot) = state.entities.slot_of_any(*id) else {
            continue;
        };
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let type_id = match body.kind {
            EntityKind::Mob { type_id }
            | EntityKind::Item { type_id }
            | EntityKind::ExperienceOrb { type_id } => type_id,
            EntityKind::Player { .. } => continue,
        };
        let chunk = chunk_position(state, slot);
        let image = Arc::new(crate::journal::EntityStateImage::from_row(body, row));
        state
            .journaled_entity_state
            .insert(body.id, (output.tick, chunk, Arc::clone(&image)));
        entries.push(JournalEntry::Entity {
            id: entity_id_int(body.id),
            uuid: row.uuid.to_string(),
            type_id,
            pos: state.entities.pos(slot),
            vel: state.entities.vel(slot),
            yaw: state.entities.rot(slot)[0],
            pitch: state.entities.rot(slot)[1],
            health: row.health,
            alive: body.alive,
            age: row.age,
            breed_cooldown: row.breed_cooldown,
            pickup_delay: body.pickup_delay,
            item: row
                .item
                .as_deref()
                .map(crate::entities::StackRow::full_stack),
            appearance: crate::journal::EntityAppearanceImage::from(&*row.appearance),
            active_effects: row.active_effects.clone(),
            villager: row
                .villager
                .as_deref()
                .map(|villager| Box::new(crate::villager::persistence::journal_image(villager))),
            state: image,
            projectile: row.projectile.clone(),
            tick: output.tick,
        });
        if let Some(previous) = state.journal_scan.motion.get_mut(id) {
            let [yaw, pitch, _] = state.entities.rot(slot);
            *previous = Some(MotionPose {
                position: state.entities.pos(slot),
                velocity: state.entities.vel(slot),
                rotation: [yaw, pitch],
            });
        }
    }
    let mut candidates = touched;
    candidates.append(&mut state.journal_scan.clock_recheck);
    candidates.sort_unstable();
    candidates.dedup();
    let mut recheck = std::mem::take(&mut state.journal_scan.clock_recheck);
    for &id in &candidates {
        let Some(slot) = state.entities.slot_of_any(id) else {
            continue;
        };
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if matches!(body.kind, EntityKind::Player { .. }) {
            continue;
        }
        let chunk = chunk_position(state, slot);
        let mut baseline = state.journaled_entity_state.entry(body.id);
        if let std::collections::hash_map::Entry::Occupied(previous) = &mut baseline {
            let (tick, old_chunk, old) = previous.get_mut();
            *old_chunk = chunk;
            if let EntityKind::Mob { type_id } = body.kind
                && old.clock_sensitive(type_id, row.appearance.baby)
                && let Some((from, to)) = state.world.simulation_interval(chunk, *tick, output.tick)
            {
                Arc::make_mut(old).advance_clocks(type_id, row.appearance.baby, to - from);
            }
            *tick = output.tick;
            if old.matches(body, row) {
                if clock_sensitive(old, body, row) {
                    recheck.push(id);
                }
                continue;
            }
        }
        let image = Arc::new(crate::journal::EntityStateImage::from_row(body, row));
        if full.binary_search(&body.id).is_err() {
            entries.push(JournalEntry::EntityState {
                id: entity_id_int(body.id),
                state: Arc::clone(&image),
                tick: output.tick,
            });
        }
        if clock_sensitive(&image, body, row) {
            recheck.push(id);
        }
        baseline.insert_entry((output.tick, chunk, image));
    }
    state.journal_scan.clock_recheck = recheck;
    state.entities.recycle_touched(candidates);
}

fn clock_sensitive(
    image: &crate::journal::EntityStateImage,
    body: &crate::entities::Body,
    row: &crate::entities::ColdRow,
) -> bool {
    !body.alive
        && matches!(body.kind, EntityKind::Mob { type_id } if image.clock_sensitive(type_id, row.appearance.baby))
}

/// Whether a villager's journaled `brain_clock` admission is on.
fn brain_clock(body: &crate::entities::Body, row: &crate::entities::ColdRow, active: bool) -> bool {
    let Some(villager) = &row.villager else {
        return false;
    };
    body.alive
        && active
        && (villager.wandering || villager.conversion_time.is_some() || !row.controls.no_ai)
}

fn chunk_position(state: &TickState, slot: EntitySlot) -> crate::boundary::ChunkPos {
    let pos = state.entities.pos(slot);
    crate::boundary::ChunkPos {
        x: (pos[0].floor() as i32).div_euclid(16),
        z: (pos[2].floor() as i32).div_euclid(16),
    }
}

/// Refresh the derived `brain_clock` flag, compare the villager with its
/// projected image and queue a full image when they differ.
fn villager_image(state: &mut TickState, slot: EntitySlot, tick: u64, full: &mut Vec<EntityId>) {
    if state.entities.row(slot).villager.is_none() {
        return;
    }
    let chunk = chunk_position(state, slot);
    let body = *state.entities.body(slot);
    // Derived flag, not an owner mutation: must not list the row as touched.
    let row = state.entities.row_mut_unmarked(slot);
    let clock = brain_clock(&body, row, state.world.is_active(chunk.x, chunk.z));
    let Some(villager) = &mut row.villager else {
        return;
    };
    villager.brain_clock = clock;
    if let Some((from, old)) = state.journaled_villagers.get_mut(&body.id) {
        if let Some((start, end)) = state.world.simulation_interval(chunk, *from, tick) {
            crate::villager::persistence::advance_clocks(old, start, end);
        }
        *from = tick;
        if crate::villager::persistence::journal_eq(villager, old) {
            return;
        }
    }
    full.push(body.id);
    state.journaled_villagers.insert(
        body.id,
        (tick, crate::villager::persistence::journal_image(villager)),
    );
}

/// Differential fence (tests only): after every touched-only batch a full scan
/// must find nothing left to emit, i.e. skipping untouched rows was exact.
#[cfg(test)]
pub(crate) fn assert_baselines_complete(state: &TickState, tick: u64) {
    for slot in state.entities.slots() {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        let chunk = chunk_position(state, slot);
        if let Some(villager) = &row.villager {
            let (from, old) = state
                .journaled_villagers
                .get(&body.id)
                .unwrap_or_else(|| panic!("villager {:?} has no journal baseline", body.id));
            let mut old = old.clone();
            if let Some((start, end)) = state.world.simulation_interval(chunk, *from, tick) {
                crate::villager::persistence::advance_clocks(&mut old, start, end);
            }
            let mut live = villager.clone();
            live.brain_clock = brain_clock(body, row, state.world.is_active(chunk.x, chunk.z));
            assert!(
                crate::villager::persistence::journal_eq(&live, &old),
                "untouched villager {:?} differs from its projected journal image",
                body.id
            );
        }
        if matches!(body.kind, EntityKind::Player { .. }) {
            continue;
        }
        let (from, _, old) = state
            .journaled_entity_state
            .get(&body.id)
            .unwrap_or_else(|| panic!("entity {:?} has no state baseline", body.id));
        let mut old = old.as_ref().clone();
        if let EntityKind::Mob { type_id } = body.kind
            && let Some((start, end)) = state.world.simulation_interval(chunk, *from, tick)
        {
            old.advance_clocks(type_id, row.appearance.baby, end - start);
        }
        assert!(
            old.matches(body, row),
            "untouched entity {:?} differs from its projected state image",
            body.id
        );
    }
}

#[cfg(test)]
#[path = "journal_touched_tests.rs"]
mod tests;
