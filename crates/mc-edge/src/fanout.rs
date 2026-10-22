//! Slow-consumer fanout: [`TickOutput`] -> per-connection framed bytes.
//!
//! Split traffic classes:
//!
//! - **World state** (chunk packets, entity spawns/moves/removes, block
//!   deltas, one-shot visuals): best-effort, planned within each viewer's
//!   world-queue credit so nothing planned is ever dropped. On lag the pump
//!   skips to the latest tick and reconciles each viewer's held world view
//!   against it; stale world packets are simply never sent.
//! - **Player-change-derived packets** (join/sync, acks, menus, cursor,
//!   health, experience, chat, game-mode): NEVER skipped. They route to a
//!   separate per-connection **reliable queue** at the server layer, which
//!   is in-order and bounded; overflow disconnects that connection rather
//!   than stalling the tick or dropping a change.
//!
//! `publish` plans with `rayon` (one viewer per thread); each packet is
//! framed once at plan time with threshold-256 compression, and encoded
//! chunk frames are shared across viewers by refcount.
//! The adaptation point for a future reliable-channel split in
//! `TickOutput` delivery is here: the caller already routes by
//! [`Priority`], so a split source only changes the input side.

use std::collections::HashMap;

use mc_core::boundary::{ConnectionId, TickOutput};
use rayon::prelude::*;

use crate::plan::{
    ChunkPacketCache, PlanRegistries, PlannedPacket, TickPlanIndex, ViewerState,
    plan_viewer_indexed,
};

/// Per-connection outbound queue bound (both reliable and world queues).
pub const OUTBOUND_QUEUE_BOUND: usize = 512;

/// Routing priority for one framed packet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Priority {
    /// Never dropped; overflow disconnects the connection.
    Reliable,
    /// World queue; planned within its free slots, so it never overflows.
    BestEffort,
}

/// One framed, addressed packet ready for the per-connection queues.
#[derive(Debug, Clone)]
pub struct AddressedPacket {
    pub connection: ConnectionId,
    pub data: bytes::Bytes,
    pub priority: Priority,
}

/// Plan + encode one tick for every viewer, in parallel.
///
/// `viewers` is consumed as an iterator of `&mut ViewerState` (the server
/// passes `viewers.values_mut()`); each viewer plans against the same
/// [`TickOutput`] on its own rayon thread. `view_distance` is the server
/// limit; negotiated per-connection radii take precedence within that limit.
///
/// Chunk encoding needs block/biome registries; the 3-arg server call site
/// carries none, so `publish` uses the process-wide required-reports
/// fallback (`solaris_required_*`). Biomes not in the fallback (custom
/// datapack biomes) skip that chunk rather than failing the tick.
pub fn publish<'a>(
    output: &TickOutput,
    viewers: impl Iterator<Item = &'a mut ViewerState> + Send,
    view_distance: i32,
) -> Vec<AddressedPacket> {
    use std::sync::LazyLock;
    struct Fallback {
        biomes: mc_data::Registry,
        blocks: mc_world::BlockRegistry,
        items: mc_data::items::ItemRegistry,
        block_entity_types: mc_data::block_entity_types::BlockEntityTypeRegistry,
    }
    static FALLBACK: LazyLock<Fallback> = LazyLock::new(|| {
        let biomes = mc_data::Registry {
            id: mc_data::Identifier::parse("minecraft:worldgen/biome").expect("static id parses"),
            entries: vec![
                mc_data::Identifier::parse("minecraft:plains").expect("static id parses"),
            ],
        };
        let blocks = mc_world::BlockRegistry::from_report(
            &mc_data::blocks::solaris_required_blocks_report(),
        )
        .expect("required blocks report builds");
        Fallback {
            biomes,
            blocks,
            items: mc_data::items::solaris_required_items(),
            block_entity_types: mc_data::block_entity_types::solaris_required_block_entity_types(),
        }
    });
    let registries = PlanRegistries {
        biomes: &FALLBACK.biomes,
        blocks: &FALLBACK.blocks,
        items: &FALLBACK.items,
        block_entity_types: &FALLBACK.block_entity_types,
    };
    // Fallback path has no shared cache (tests/single calls); per-call
    // cache still dedups viewers within this publish.
    let mut cache = ChunkPacketCache::new();
    publish_with_registries(output, viewers, view_distance, &registries, &mut cache)
}

/// [`publish`] with explicit registries (tests + future server wiring).
/// `chunk_cache` is owned by the caller across publishes: framed chunk
/// packets (including authoritative baked light) are reused by revision.
/// Viewers plan in parallel against the read-only cache; misses are then
/// encoded once per chunk and filled in (`plan::fill_chunk_packets`).
pub fn publish_with_registries<'a>(
    output: &TickOutput,
    viewers: impl Iterator<Item = &'a mut ViewerState> + Send,
    view_distance: i32,
    registries: &PlanRegistries<'_>,
    chunk_cache: &mut crate::plan::ChunkPacketCache,
) -> Vec<AddressedPacket> {
    chunk_cache.retain_resident(output);
    chunk_cache.deltas.begin();
    // One delta per dimension, even if the root also lists its own projection.
    let mut chunk_deltas: HashMap<_, Option<std::sync::Arc<_>>> = HashMap::new();
    for projection in std::iter::once(output).chain(output.snapshot.dimensions.values()) {
        chunk_deltas
            .entry(projection.snapshot.dimension)
            .or_insert_with(|| {
                chunk_cache
                    .deltas
                    .delta(projection)
                    .map(std::sync::Arc::new)
            });
    }
    let mut viewers: Vec<&mut ViewerState> = viewers.collect();
    let cache = &*chunk_cache;
    let indices: HashMap<_, _> = std::iter::once(output)
        .chain(output.snapshot.dimensions.values())
        .map(|projection| {
            let dimension = projection.snapshot.dimension;
            let entities = &projection.snapshot.entities;
            let previous = viewers
                .iter()
                .filter(|viewer| viewer.dimension == dimension)
                .filter_map(|viewer| viewer.previous_entities())
                .find(|previous| previous.revision() == entities.base_revision())
                .or_else(|| {
                    viewers
                        .iter()
                        .filter(|viewer| viewer.dimension == dimension)
                        .filter_map(|viewer| viewer.previous_entities())
                        .find(|previous| previous.revision() == entities.revision())
                });
            (
                dimension,
                TickPlanIndex::with_previous(projection, previous)
                    .with_reveals(projection, cache.anti_xray())
                    .with_chunk_delta(chunk_deltas.get(&dimension).cloned().flatten()),
            )
        })
        .collect();
    let mut planned: Vec<(ConnectionId, Vec<PlannedPacket>)> = viewers
        .par_iter_mut()
        .map(|viewer| {
            let dimension = viewer.dimension;
            let packets = output
                .in_dimension(dimension)
                .map_or_else(Vec::new, |projection| {
                    plan_viewer_indexed(
                        projection,
                        viewer,
                        view_distance,
                        cache,
                        &indices[&dimension],
                    )
                });
            (viewer.connection, packets)
        })
        .collect();
    let failed = crate::plan::fill_chunk_packets(output, &mut planned, registries, chunk_cache);
    for (connection, pos) in failed {
        if let Some(viewer) = viewers
            .iter_mut()
            .find(|viewer| viewer.connection == connection)
        {
            viewer.chunk_unsent(pos);
        }
    }
    let mut addressed = Vec::new();
    for (connection, packets) in planned {
        for packet in packets {
            addressed.push(AddressedPacket {
                connection,
                data: packet.data,
                priority: if packet.reliable {
                    Priority::Reliable
                } else {
                    Priority::BestEffort
                },
            });
        }
    }
    addressed
}

/// Connections whose reliable output exceeds the bound. The caller must
/// disconnect them: reliable packets are never skipped, so an over-full
/// reliable queue means the consumer is gone.
///
/// Counts framed reliable bytes per connection; any connection with more
/// than [`OUTBOUND_QUEUE_BOUND`] queued reliable packets overflows. (The
/// live queue depth is owned by the server layer; this pure helper lets
/// the pump decide from a single `publish` batch.)
pub fn reliable_overflow(packets: &[AddressedPacket]) -> Vec<ConnectionId> {
    let mut counts: HashMap<ConnectionId, usize> = HashMap::new();
    for packet in packets.iter() {
        if packet.priority == Priority::Reliable {
            *counts.entry(packet.connection).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .filter(|(_, count)| *count > OUTBOUND_QUEUE_BOUND)
        .map(|(connection, _)| connection)
        .collect()
}

#[cfg(test)]
#[path = "fanout_tests.rs"]
mod fanout_tests;
