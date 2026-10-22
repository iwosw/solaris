//! Derived chunk tickets and the shared residency/simulation extents.
//!
//! Ender pearls (26.1.2 `ThrownEnderpearl.tick` -> `ServerPlayer.
//! registerAndUpdateEnderPearlTicket` -> `TicketType.ENDER_PEARL`): while the
//! owning player is online a pearl holds a level-31 ticket (centre entity
//! ticking, loading radius two) on its current chunk. The ticket times out
//! 40 ticks after it was last refreshed, is never persisted, and survives the
//! owner changing dimension. A pearl whose owner is offline is vanilla's
//! "saved with the owner": native entities already persist in the checkpoint,
//! so such a pearl is parked (see [`parked`]) and resumes when the owner joins.
//!
//! The pearl set per owner is capped ([`MAX_CHUNKS_PER_OWNER`]) so a pearl
//! cannon cannot demand an unbounded loaded area. Durable tickets (forced
//! chunks, API/plugin tickets) stay in [`TickState::tickets`]; this module also
//! diffs them into the WAL so removals are recorded.

use std::collections::BTreeMap;

use rustc_hash::{FxHashMap, FxHashSet};
use uuid::Uuid;

use crate::boundary::ChunkPos;
use crate::entities::EntityKind;
use crate::journal::JournalEntry;
use crate::projectiles::{ProjectileKind, ProjectileState};
use crate::tick::TickState;

/// `TicketType.ENDER_PEARL` timeout in ticks.
const TIMEOUT_TICKS: u64 = 40;
/// Distinct chunks one owner's pearls may ticket at once. Excess pearls simply
/// stay frozen outside simulation instead of widening the loaded area.
pub(crate) const MAX_CHUNKS_PER_OWNER: usize = 32;

#[derive(Debug, Default)]
pub(crate) struct ChunkTickets {
    /// Cached blueprint id of `minecraft:ender_pearl` (`Some(None)`: absent).
    pearl_type: Option<Option<u32>>,
    /// Pearl ticket centre -> first tick it no longer holds.
    held: FxHashMap<ChunkPos, u64>,
    /// `held` keys, sorted so equality checks and iteration are stable.
    pearl_chunks: Vec<ChunkPos>,
    /// Players online in the other dimensions, published by the coordinator.
    remote_online: FxHashSet<Uuid>,
    /// Durable ticket image last written to the WAL.
    journaled: BTreeMap<String, (ChunkPos, i32)>,
}

impl ChunkTickets {
    /// Sorted centres of the live pearl tickets.
    pub(crate) fn pearl_chunks(&self) -> &[ChunkPos] {
        &self.pearl_chunks
    }

    /// Replace the set of owners online in dimensions other than this one.
    pub(crate) fn set_remote_online(&mut self, online: &FxHashSet<Uuid>) {
        self.remote_online.clone_from(online);
    }
}

impl TickState {
    /// Every ticket as `(centre, loading radius, simulation radius)`.
    ///
    /// Forced chunks and pearls use vanilla's level-31 extents; every other
    /// ticket uses its single radius for both.
    pub(crate) fn ticket_areas(&self) -> impl Iterator<Item = (ChunkPos, i32, i32)> + '_ {
        self.tickets
            .iter()
            .map(|(owner, &(centre, radius))| {
                if crate::forceload::is_forced_owner(owner) {
                    (
                        centre,
                        crate::forceload::LOAD_RADIUS,
                        crate::forceload::SIM_RADIUS,
                    )
                } else {
                    (centre, radius, radius)
                }
            })
            .chain(self.chunk_tickets.pearl_chunks().iter().map(|&centre| {
                (
                    centre,
                    crate::forceload::LOAD_RADIUS,
                    crate::forceload::SIM_RADIUS,
                )
            }))
    }
}

/// Whether the player `owner` is connected to any dimension.
pub(crate) fn owner_online(state: &TickState, owner: Uuid) -> bool {
    state
        .entities
        .slot_of_uuid_any(owner)
        .is_some_and(|slot| matches!(state.entities.body(slot).kind, EntityKind::Player { .. }))
        || state.chunk_tickets.remote_online.contains(&owner)
}
/// Tell every world which players are online anywhere, so a pearl ticket
/// outlives its owner changing dimension. Single-world servers need nothing:
/// [`owner_online`] already sees their local players.
pub(crate) fn publish_online(root: &mut TickState) {
    let mut online = FxHashSet::default();
    for world in std::iter::once(&*root).chain(root.other_dimensions.values().map(|w| &**w)) {
        online.extend(
            world
                .connections
                .all_connections()
                .filter_map(|connection| world.connections.uuid_of(connection)),
        );
    }
    root.chunk_tickets.set_remote_online(&online);
    for world in root.other_dimensions.values_mut() {
        world.chunk_tickets.set_remote_online(&online);
    }
}

/// A player-owned pearl whose owner is offline: it neither flies, lands, nor
/// ages until the owner joins again.
pub(crate) fn parked(state: &TickState, record: &ProjectileState) -> bool {
    record.kind == ProjectileKind::EnderPearl
        && record
            .owner
            .is_some_and(|owner| !owner_online(state, owner))
}

/// Recompute the pearl tickets for the tick about to run (`state.tick + 1`),
/// before residency and the active set are derived from them.
pub(crate) fn refresh(state: &mut TickState) {
    let tick = state.tick + 1;
    let type_id = match state.chunk_tickets.pearl_type {
        Some(type_id) => type_id,
        None => {
            let type_id = mc_data::Identifier::parse("minecraft:ender_pearl")
                .ok()
                .and_then(|name| state.registry.entity_types.id_of(&name));
            state.chunk_tickets.pearl_type = Some(type_id);
            type_id
        }
    };
    let mut live: Vec<ChunkPos> = Vec::new();
    if let Some(type_id) = type_id {
        let mut per_owner: FxHashMap<Uuid, Vec<ChunkPos>> = FxHashMap::default();
        for slot in state.entities.mob_slots(type_id) {
            if !state.entities.body(slot).alive {
                continue;
            }
            let Some(record) = state.entities.row(slot).projectile.as_deref() else {
                continue;
            };
            let Some(owner) = record.owner else {
                continue;
            };
            if !owner_online(state, owner) {
                continue;
            }
            let pos = state.entities.pos(slot);
            let chunk = ChunkPos {
                x: mc_physics::mth::floor_i32(pos[0]).div_euclid(16),
                z: mc_physics::mth::floor_i32(pos[2]).div_euclid(16),
            };
            let chunks = per_owner.entry(owner).or_default();
            if chunks.len() < MAX_CHUNKS_PER_OWNER && !chunks.contains(&chunk) {
                chunks.push(chunk);
                live.push(chunk);
            }
        }
    }
    let tickets = &mut state.chunk_tickets;
    if live.is_empty() && tickets.held.is_empty() {
        return;
    }
    let mut changed = false;
    for chunk in live {
        changed |= tickets.held.insert(chunk, tick + TIMEOUT_TICKS).is_none();
    }
    let before = tickets.held.len();
    tickets.held.retain(|_, until| *until > tick);
    changed |= tickets.held.len() != before;
    if changed {
        tickets.pearl_chunks.clear();
        tickets.pearl_chunks.extend(tickets.held.keys().copied());
        tickets
            .pearl_chunks
            .sort_unstable_by_key(|chunk| (chunk.x, chunk.z));
    }
}

/// Append the WAL records for durable ticket changes since the last call:
/// new or moved tickets as post-images, vanished ones as tombstones.
pub(crate) fn journal(state: &mut TickState, tick: u64, entries: &mut Vec<JournalEntry>) {
    let removed: Vec<String> = state
        .chunk_tickets
        .journaled
        .keys()
        .filter(|owner| !state.tickets.contains_key(*owner))
        .cloned()
        .collect();
    for owner in removed {
        state.chunk_tickets.journaled.remove(&owner);
        entries.push(JournalEntry::TicketRemoved { owner, tick });
    }
    for (owner, &(centre, radius)) in &state.tickets {
        // Portal transfer tickets are transient and rebuilt by their owners.
        if owner.starts_with("solaris:portal:")
            || state.chunk_tickets.journaled.get(owner) == Some(&(centre, radius))
        {
            continue;
        }
        state
            .chunk_tickets
            .journaled
            .insert(owner.clone(), (centre, radius));
        entries.push(JournalEntry::Ticket {
            owner: owner.clone(),
            chunk: [centre.x, centre.z],
            tick,
            radius,
        });
    }
}

#[cfg(test)]
#[path = "chunk_tickets_tests.rs"]
mod tests;
