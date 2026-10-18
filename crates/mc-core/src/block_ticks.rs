//! Native scheduled block work. Queue mutations share the committing tick's
//! WAL batch; absolute deadlines and insertion identities ride the checkpoint
//! manifest, while region chunks also retain vanilla `block_ticks` records.

use std::collections::{BTreeMap, BTreeSet};

use mc_data::Identifier;
use mc_world::{BlockPos, Chunk, ScheduledBlockTick};
use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

type Order = (u64, i32, u64);
type Column = (i32, i32);

/// Native absolute post-image. Vanilla NBT uses i/x/y/z/t/p instead; its
/// sequence is list order, as implemented by mc-world's existing Anvil codec.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTickImage {
    pub pos: [i32; 3],
    pub block: Identifier,
    pub trigger_tick: u64,
    pub priority: i32,
    pub sequence: u64,
}

impl BlockTickImage {
    fn from_tick(tick: &ScheduledBlockTick) -> Self {
        Self {
            pos: [tick.pos.x, tick.pos.y, tick.pos.z],
            block: tick.block.clone(),
            trigger_tick: tick.trigger_tick,
            priority: tick.priority,
            sequence: tick.sequence(),
        }
    }

    fn into_tick(self) -> ScheduledBlockTick {
        ScheduledBlockTick::from_storage(
            BlockPos {
                x: self.pos[0],
                y: self.pos[1],
                z: self.pos[2],
            },
            self.block,
            self.trigger_tick,
            self.priority,
            self.sequence,
        )
    }
}

/// Empty chunk images are tombstones too: a later reload must not import an
/// already consumed tick from an older region chunk.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTickChunkImage {
    pub chunk: [i32; 2],
    pub ticks: Vec<BlockTickImage>,
}

#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockTicksImage {
    pub next_sequence: u64,
    pub chunks: Vec<BlockTickChunkImage>,
}

impl BlockTicksImage {
    /// Check the invariants required by the due map and its cell-local index
    /// before adopting a durable image. Native imports assign fresh identities.
    pub fn validate(&self) -> Result<(), crate::config::CoreError> {
        let failure =
            |detail: String| crate::config::CoreError::Storage(format!("block_ticks: {detail}"));
        if self.next_sequence == u64::MAX {
            return Err(failure("next_sequence has no scheduling headroom".into()));
        }
        let mut previous = None;
        let mut orders = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut identities = rustc_hash::FxHashSet::default();
        for chunk in &self.chunks {
            if previous.is_some_and(|key| key >= chunk.chunk) {
                return Err(failure(format!(
                    "chunk {:?}: coverage is not strictly ordered",
                    chunk.chunk
                )));
            }
            previous = Some(chunk.chunk);
            for tick in &chunk.ticks {
                let context = || format!("chunk {:?}, cell {:?}", chunk.chunk, tick.pos);
                if [tick.pos[0].div_euclid(16), tick.pos[2].div_euclid(16)] != chunk.chunk {
                    return Err(failure(format!("{}: outside chunk coverage", context())));
                }
                if tick.sequence >= self.next_sequence {
                    return Err(failure(format!(
                        "{}: sequence {} is not below next_sequence {}",
                        context(),
                        tick.sequence,
                        self.next_sequence
                    )));
                }
                let order = (tick.trigger_tick, tick.priority, tick.sequence);
                if !orders.insert(order) {
                    return Err(failure(format!("{}: duplicate order {order:?}", context())));
                }
                if !sequences.insert(tick.sequence) {
                    return Err(failure(format!(
                        "{}: duplicate sequence {}",
                        context(),
                        tick.sequence
                    )));
                }
                if !identities.insert((tick.pos, &tick.block, tick.trigger_tick, tick.priority)) {
                    return Err(failure(format!("{}: duplicate tick identity", context())));
                }
            }
        }
        Ok(())
    }

    /// Fold an absolute chunk image; replay is idempotent and includes clears.
    pub(crate) fn apply(&mut self, image: BlockTickChunkImage, next_sequence: u64) {
        match self
            .chunks
            .binary_search_by_key(&image.chunk, |entry| entry.chunk)
        {
            Ok(index) => self.chunks[index] = image,
            Err(index) => self.chunks.insert(index, image),
        }
        self.next_sequence = self.next_sequence.max(next_sequence);
    }
}

#[derive(Debug, Default)]
pub struct BlockTicks {
    due: BTreeMap<Order, ScheduledBlockTick>,
    // Cell-local identities avoid cloning identifier strings into a dedup index.
    by_cell: FxHashMap<BlockPos, Vec<Order>>,
    covered: BTreeSet<Column>,
    dirty: BTreeSet<Column>,
    /// Native projection backlog, independent of WAL `dirty`. `checkpoint()`
    /// projects only these chunks (including explicit empty clears) and never
    /// clears them, so a failed capture retries; only a successful
    /// `acknowledge_flushed` against a matching captured image does. Journal
    /// emission never touches this set.
    checkpoint_dirty: BTreeSet<Column>,
    /// Only checkpoint-acknowledged empty Anvil queues may lose coverage.
    flushed_empty: BTreeSet<Column>,
    next_sequence: u64,
    selected: Vec<Order>,
}

fn column(pos: BlockPos) -> Column {
    (pos.x.div_euclid(16), pos.z.div_euclid(16))
}

fn order(tick: &ScheduledBlockTick) -> Order {
    (tick.trigger_tick, tick.priority, tick.sequence())
}

impl BlockTicks {
    /// Rebuild the queue from a complete replay image. `checkpoint` is the
    /// durable manifest image: chunks semantically equal to it (compared per
    /// chunk, ignoring tick order within a chunk) start native-checkpoint
    /// clean, while new or changed chunks start pending. `None` proves no
    /// durable projection, so every covered chunk starts pending. Empty
    /// coverage known-durable (equal to base) starts acknowledged for
    /// eviction; `next_sequence` alone never dirties a chunk.
    #[must_use]
    pub fn from_image(image: BlockTicksImage, checkpoint: Option<&BlockTicksImage>) -> Self {
        let mut queue = Self {
            next_sequence: image.next_sequence,
            ..Self::default()
        };
        match checkpoint {
            None => {
                for chunk in &image.chunks {
                    queue
                        .checkpoint_dirty
                        .insert((chunk.chunk[0], chunk.chunk[1]));
                }
            }
            Some(base) => {
                for chunk in &image.chunks {
                    let column = (chunk.chunk[0], chunk.chunk[1]);
                    let durable = match base
                        .chunks
                        .binary_search_by_key(&chunk.chunk, |entry| entry.chunk)
                    {
                        Ok(index) => {
                            Self::chunk_ticks_equal(&chunk.ticks, &base.chunks[index].ticks)
                        }
                        Err(_) => false,
                    };
                    if durable {
                        if chunk.ticks.is_empty() {
                            queue.flushed_empty.insert(column);
                        }
                    } else {
                        queue.checkpoint_dirty.insert(column);
                    }
                }
            }
        }
        for chunk in image.chunks {
            queue.covered.insert((chunk.chunk[0], chunk.chunk[1]));
            for image in chunk.ticks {
                queue.insert_identity(image.into_tick());
            }
        }
        queue
    }

    /// Order-insensitive per-chunk queue equality. Chunk lists are strictly
    /// ordered, but ticks within a chunk are only unique (order/sequence/
    /// identity), so semantic comparison matches by order with borrowed
    /// full-tick checks instead of raw `Vec` order.
    fn chunk_ticks_equal(current: &[BlockTickImage], base: &[BlockTickImage]) -> bool {
        if current == base {
            return true;
        }
        if current.len() != base.len() {
            return false;
        }
        let mut by_order: FxHashMap<Order, &BlockTickImage> =
            FxHashMap::with_capacity_and_hasher(base.len(), Default::default());
        for tick in base {
            if by_order
                .insert((tick.trigger_tick, tick.priority, tick.sequence), tick)
                .is_some()
            {
                return false;
            }
        }
        current.iter().all(|tick| {
            by_order
                .get(&(tick.trigger_tick, tick.priority, tick.sequence))
                .is_some_and(|base| *base == tick)
        })
    }

    fn insert_identity(&mut self, tick: ScheduledBlockTick) {
        let key = order(&tick);
        self.by_cell.entry(tick.pos).or_default().push(key);
        self.due.insert(key, tick);
    }

    /// Component transition guards may suppress a pending kind independently
    /// of its deadline. The generic queue itself still uses exact dedup.
    #[must_use]
    pub(crate) fn contains(&self, pos: BlockPos, block: &Identifier, priority: i32) -> bool {
        self.by_cell.get(&pos).is_some_and(|keys| {
            keys.iter()
                .any(|key| key.1 == priority && &self.due[key].block == block)
        })
    }

    /// Exact native dedup semantics: cell, block, deadline, and priority. A
    /// rejected duplicate consumes no insertion sequence.
    pub fn schedule(&mut self, tick: ScheduledBlockTick) -> bool {
        if self.by_cell.get(&tick.pos).is_some_and(|keys| {
            keys.iter().any(|key| {
                let queued = &self.due[key];
                queued.block == tick.block
                    && queued.trigger_tick == tick.trigger_tick
                    && queued.priority == tick.priority
            })
        }) {
            return false;
        }
        let cell = tick.pos;
        self.flushed_empty.remove(&column(cell));
        let tick = ScheduledBlockTick::from_storage(
            tick.pos,
            tick.block,
            tick.trigger_tick,
            tick.priority,
            self.next_sequence,
        );
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("block tick sequence exhausted");
        self.insert_identity(tick);
        self.covered.insert(column(cell));
        self.dirty.insert(column(cell));
        self.checkpoint_dirty.insert(column(cell));
        true
    }

    /// Import vanilla delays only once. Covered queues (including empty ones)
    /// are already authoritative and survive residency/eviction unchanged.
    pub fn hydrate_chunk(&mut self, chunk: &Chunk, now: u64) {
        let key = (chunk.pos.x, chunk.pos.z);
        if self.covered.contains(&key) || chunk.scheduled_block_ticks().is_empty() {
            return;
        }
        // mc-world decodes Anvil trigger_tick against zero. Preserve its
        // deadline/priority/list ordering while assigning global identities.
        for tick in chunk.scheduled_block_ticks() {
            self.schedule(ScheduledBlockTick::new(
                tick.pos,
                tick.block.clone(),
                now.saturating_add(tick.trigger_tick),
                tick.priority,
            ));
        }
    }

    pub fn drain_loaded(
        &mut self,
        now: u64,
        limit: usize,
        world: &crate::world::ChunkWorld,
    ) -> Vec<ScheduledBlockTick> {
        self.drain_ready(now, limit, |pos| {
            world.is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
                && world.block_at(pos.x, pos.y, pos.z).is_some()
        })
    }

    /// Unavailable cells do not consume the budget or change identity/order.
    pub fn drain_ready(
        &mut self,
        now: u64,
        limit: usize,
        mut available: impl FnMut(BlockPos) -> bool,
    ) -> Vec<ScheduledBlockTick> {
        if limit == 0 {
            return Vec::new();
        }
        self.selected.clear();
        self.selected.extend(
            self.due
                .iter()
                .take_while(|(key, _)| key.0 <= now)
                .filter(|(_, tick)| available(tick.pos))
                .take(limit)
                .map(|(key, _)| *key),
        );
        let mut result = Vec::with_capacity(self.selected.len());
        for key in self.selected.drain(..) {
            let tick = self
                .due
                .remove(&key)
                .expect("selected block tick remains queued");
            let keys = self
                .by_cell
                .get_mut(&tick.pos)
                .expect("queued block tick cell");
            keys.retain(|queued| *queued != key);
            if keys.is_empty() {
                self.by_cell.remove(&tick.pos);
            }
            self.dirty.insert(column(tick.pos));
            self.checkpoint_dirty.insert(column(tick.pos));
            result.push(tick);
        }
        result
    }

    /// Snapshot all covered chunk queues, including cleared queues. Entries
    /// and chunks are canonically ordered for reproducible durable images.
    #[must_use]
    pub fn image(&self) -> BlockTicksImage {
        let mut chunks: BTreeMap<Column, Vec<BlockTickImage>> =
            self.covered.iter().map(|key| (*key, Vec::new())).collect();
        for tick in self.due.values() {
            chunks
                .get_mut(&column(tick.pos))
                .expect("queued block tick coverage")
                .push(BlockTickImage::from_tick(tick));
        }
        BlockTicksImage {
            next_sequence: self.next_sequence,
            chunks: chunks
                .into_iter()
                .map(|((x, z), ticks)| BlockTickChunkImage {
                    chunk: [x, z],
                    ticks,
                })
                .collect(),
        }
    }

    /// Consume changed chunk post-images into the same journal batch as the
    /// block/container mutations that scheduled or consumed them.
    pub(crate) fn journal_entries(&mut self, entries: &mut Vec<crate::journal::JournalEntry>) {
        if self.dirty.is_empty() {
            return;
        }
        let mut chunks: BTreeMap<Column, Vec<BlockTickImage>> = std::mem::take(&mut self.dirty)
            .into_iter()
            .map(|key| (key, Vec::new()))
            .collect();
        for tick in self.due.values() {
            if let Some(ticks) = chunks.get_mut(&column(tick.pos)) {
                ticks.push(BlockTickImage::from_tick(tick));
            }
        }
        for ((x, z), ticks) in chunks {
            entries.push(crate::journal::JournalEntry::BlockTicks {
                image: BlockTickChunkImage {
                    chunk: [x, z],
                    ticks,
                },
                next_sequence: self.next_sequence,
            });
        }
    }

    /// The manifest is the restart authority; regions carry the same queue
    /// using vanilla fields for interoperability, never opaque block entities.
    /// Only block work is rebased here: chunk fluid work already uses the
    /// Anvil zero-based delay clock and must not be shifted by this scheduler.
    /// Projects only the native-projection backlog (including explicit empty
    /// clears) without clearing it: journal emission never ACKs, and a failed
    /// capture keeps pending. Retry-safe.
    pub(crate) fn checkpoint(
        &self,
        world: &mut crate::world::ChunkWorld,
        now: u64,
    ) -> Vec<(mc_world::ChunkPos, Vec<ScheduledBlockTick>)> {
        if self.checkpoint_dirty.is_empty() {
            return Vec::new();
        }
        let mut chunks: BTreeMap<Column, Vec<ScheduledBlockTick>> = self
            .checkpoint_dirty
            .iter()
            .map(|key| (*key, Vec::new()))
            .collect();
        for tick in self.due.values() {
            if let Some(ticks) = chunks.get_mut(&column(tick.pos)) {
                ticks.push(ScheduledBlockTick::from_storage(
                    tick.pos,
                    tick.block.clone(),
                    tick.trigger_tick.saturating_sub(now),
                    tick.priority,
                    tick.sequence(),
                ));
            }
        }
        let mut absent = Vec::new();
        for ((x, z), ticks) in chunks {
            let pos = mc_world::ChunkPos { x, z };
            if world.has_chunk(x, z) {
                world.set_scheduled_block_ticks(pos, ticks);
            } else {
                absent.push((pos, ticks));
            }
        }
        absent
    }

    /// Called only after the captured queues and manifest have been flushed.
    /// Clears projection backlog only where the captured per-chunk queue
    /// still equals the current one (order-insensitive within a chunk), so a
    /// stale ACK after a newer schedule/consume keeps the newer state. Empty
    /// coverage equal to the captured empty image may lose coverage; pending
    /// native projections never do.
    pub(crate) fn acknowledge_flushed(&mut self, image: &BlockTicksImage) {
        if self.checkpoint_dirty.is_empty() {
            return;
        }
        let mut current: BTreeMap<Column, Vec<&ScheduledBlockTick>> = BTreeMap::new();
        for column in self.checkpoint_dirty.iter() {
            current.insert(*column, Vec::new());
        }
        for tick in self.due.values() {
            if let Some(ticks) = current.get_mut(&column(tick.pos)) {
                ticks.push(tick);
            }
        }
        for chunk in &image.chunks {
            let column = (chunk.chunk[0], chunk.chunk[1]);
            let Some(queued) = current.get(&column) else {
                continue;
            };
            if !Self::captured_matches(queued, &chunk.ticks) {
                continue;
            }
            self.checkpoint_dirty.remove(&column);
            if chunk.ticks.is_empty() {
                // Equality already implies the current queue is empty too.
                self.flushed_empty.insert(column);
            }
        }
    }

    /// Current groups inherit due-map order, so unordered captured ticks use
    /// binary search without another comparison map or payload clone.
    fn captured_matches(current: &[&ScheduledBlockTick], captured: &[BlockTickImage]) -> bool {
        if current.len() != captured.len() {
            return false;
        }
        if current.is_empty() {
            return true;
        }
        captured.iter().all(|tick| {
            current
                .binary_search_by_key(
                    &(tick.trigger_tick, tick.priority, tick.sequence),
                    |queued| order(queued),
                )
                .ok()
                .is_some_and(|index| {
                    let queued = current[index];
                    queued.pos.x == tick.pos[0]
                        && queued.pos.y == tick.pos[1]
                        && queued.pos.z == tick.pos[2]
                        && queued.block == tick.block
                })
        })
    }

    /// Residency and world dirtiness are checked by the tick owner. WAL
    /// emission alone never authorizes removal of an empty tombstone, and a
    /// chunk with a pending native projection keeps its coverage too.
    pub(crate) fn evict_flushed(&mut self, mut unloaded_clean: impl FnMut(i32, i32) -> bool) {
        let covered = &mut self.covered;
        let dirty = &self.dirty;
        let pending = &self.checkpoint_dirty;
        self.flushed_empty.retain(|&(x, z)| {
            if dirty.contains(&(x, z)) || pending.contains(&(x, z)) || !unloaded_clean(x, z) {
                return true;
            }
            covered.remove(&(x, z));
            false
        });
    }
}

#[cfg(test)]
#[path = "block_ticks_tests.rs"]
mod tests;
