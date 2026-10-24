//! Absolute native fluid work, including unloaded queues and empty tombstones.
use super::Fluid;
use mc_data::Identifier;
use mc_world::{BlockPos, ChunkPos, ScheduledFluidTick};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, HashMap};
type Order = (u64, i32, u64);
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FluidTickImage {
    pub pos: [i32; 3],
    pub fluid: Identifier,
    pub deadline: u64,
    pub priority: i32,
    pub sequence: u64,
}
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FluidTicksImage {
    pub due: Vec<FluidTickImage>,
    pub covered: Vec<[i32; 2]>,
    pub next_sequence: u64,
}
impl FluidTicksImage {
    /// Validate both queue indexes and coverage before adopting persisted work.
    pub fn validate(&self) -> Result<(), crate::config::CoreError> {
        let failure =
            |detail: String| crate::config::CoreError::Storage(format!("fluid_ticks: {detail}"));
        if self.next_sequence == u64::MAX {
            return Err(failure("next_sequence has no scheduling headroom".into()));
        }
        let mut covered = BTreeSet::new();
        for chunk in &self.covered {
            if !covered.insert(*chunk) {
                return Err(failure(format!("chunk {chunk:?}: duplicate coverage")));
            }
        }
        let mut orders = BTreeSet::new();
        let mut sequences = BTreeSet::new();
        let mut identities = std::collections::HashSet::new();
        for tick in &self.due {
            let context = || format!("cell {:?}", tick.pos);
            if ScheduledFluidTick::fluid_kind(&tick.fluid).is_none() {
                return Err(failure(format!(
                    "{}: unsupported fluid identifier {}",
                    context(),
                    tick.fluid
                )));
            }
            if !covered.contains(&[tick.pos[0].div_euclid(16), tick.pos[2].div_euclid(16)]) {
                return Err(failure(format!("{}: missing chunk coverage", context())));
            }
            if tick.sequence >= self.next_sequence {
                return Err(failure(format!(
                    "{}: sequence {} is not below next_sequence {}",
                    context(),
                    tick.sequence,
                    self.next_sequence
                )));
            }
            let order = (tick.deadline, tick.priority, tick.sequence);
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
            if !identities.insert((tick.pos, &tick.fluid, tick.deadline, tick.priority)) {
                return Err(failure(format!("{}: duplicate tick identity", context())));
            }
        }
        Ok(())
    }
}
#[derive(Debug, Default)]
pub struct FluidTicks {
    due: BTreeMap<Order, FluidTickImage>,
    queued: HashMap<(BlockPos, Fluid, bool, u64, i32), u64>,
    covered: BTreeSet<[i32; 2]>,
    /// Native projection backlog, independent of the WAL `dirty` flag below.
    /// Every accepted schedule/hydrate/consume marks its chunk; only a
    /// matching flush acknowledgement clears it. Journal emission never does.
    checkpoint_dirty: BTreeSet<[i32; 2]>,
    /// Empty queues whose captured Anvil/manifest checkpoint was acknowledged.
    flushed_empty: BTreeSet<[i32; 2]>,
    next_sequence: u64,
    dirty: bool,
}
fn identity(id: &Identifier) -> Option<(Fluid, bool)> {
    Fluid::from_id(id).map(|fluid| (fluid, id.path().starts_with("flowing_")))
}

fn chunk_of_tick(tick: &FluidTickImage) -> [i32; 2] {
    [tick.pos[0].div_euclid(16), tick.pos[2].div_euclid(16)]
}

/// Borrowed groups preserve input order; live due-map rows stay canonical.
fn grouped_by_chunk<'a, I>(ticks: I) -> HashMap<[i32; 2], Vec<&'a FluidTickImage>>
where
    I: IntoIterator<Item = &'a FluidTickImage>,
{
    let mut groups: HashMap<[i32; 2], Vec<&'a FluidTickImage>> = HashMap::new();
    for tick in ticks {
        groups.entry(chunk_of_tick(tick)).or_default().push(tick);
    }
    groups
}

fn rows<'a>(
    groups: &'a HashMap<[i32; 2], Vec<&'a FluidTickImage>>,
    chunk: &[i32; 2],
) -> &'a [&'a FluidTickImage] {
    groups.get(chunk).map(Vec::as_slice).unwrap_or(&[])
}

/// Live groups inherit due-map order; captured/base rows may be unordered.
fn rows_equal(current: &[&FluidTickImage], captured: &[&FluidTickImage]) -> bool {
    if current == captured {
        return true;
    }
    current.len() == captured.len()
        && captured.iter().all(|tick| {
            current
                .binary_search_by_key(&(tick.deadline, tick.priority, tick.sequence), |row| {
                    (row.deadline, row.priority, row.sequence)
                })
                .is_ok_and(|index| current[index] == *tick)
        })
}

impl FluidTicks {
    pub fn image(&self) -> FluidTicksImage {
        FluidTicksImage {
            due: self.due.values().cloned().collect(),
            covered: self.covered.iter().copied().collect(),
            next_sequence: self.next_sequence,
        }
    }
    pub fn from_image(image: FluidTicksImage, checkpoint: Option<&FluidTicksImage>) -> Self {
        let mut queue = Self {
            covered: image.covered.iter().copied().collect(),
            next_sequence: image.next_sequence,
            ..Self::default()
        };
        for tick in image.due {
            let pos = BlockPos {
                x: tick.pos[0],
                y: tick.pos[1],
                z: tick.pos[2],
            };
            let order = (tick.deadline, tick.priority, tick.sequence);
            let (fluid, flowing) = identity(&tick.fluid).expect("validated fluid tick identifier");
            if queue
                .queued
                .insert(
                    (pos, fluid, flowing, tick.deadline, tick.priority),
                    tick.sequence,
                )
                .is_none()
            {
                queue.due.insert(order, tick);
            }
        }
        match checkpoint {
            // No durable projection proven: every covered queue may need one.
            None => {
                queue.checkpoint_dirty = queue.covered.clone();
            }
            Some(base) => {
                // The durable image may list chunks and ticks in any valid
                // order, so compare per-chunk rows through borrowed lookups.
                // The global sequence marker is not per-chunk content.
                let durable_covered: BTreeSet<[i32; 2]> = base.covered.iter().copied().collect();
                let durable = grouped_by_chunk(base.due.iter());
                let live = grouped_by_chunk(queue.due.values());
                for chunk in queue.covered.iter().copied() {
                    let unchanged = durable_covered.contains(&chunk)
                        && rows_equal(rows(&live, &chunk), rows(&durable, &chunk));
                    if unchanged {
                        if rows(&live, &chunk).is_empty() {
                            queue.flushed_empty.insert(chunk);
                        }
                    } else {
                        queue.checkpoint_dirty.insert(chunk);
                    }
                }
            }
        }
        queue
    }
    pub(crate) fn schedule(
        &mut self,
        pos: BlockPos,
        fluid: &Identifier,
        deadline: u64,
        priority: i32,
    ) -> bool {
        let Some((family, flowing)) = identity(fluid) else {
            return false;
        };
        if self
            .queued
            .contains_key(&(pos, family, flowing, deadline, priority))
        {
            return false;
        }
        let sequence = self.next_sequence;
        self.flushed_empty
            .remove(&[pos.x.div_euclid(16), pos.z.div_euclid(16)]);
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .expect("fluid tick sequence exhausted");
        let order = (deadline, priority, sequence);
        self.queued
            .insert((pos, family, flowing, deadline, priority), sequence);
        self.due.insert(
            order,
            FluidTickImage {
                pos: [pos.x, pos.y, pos.z],
                fluid: fluid.clone(),
                deadline,
                priority,
                sequence,
            },
        );
        self.covered
            .insert([pos.x.div_euclid(16), pos.z.div_euclid(16)]);
        self.checkpoint_dirty
            .insert([pos.x.div_euclid(16), pos.z.div_euclid(16)]);
        self.dirty = true;
        true
    }
    pub(crate) fn hydrate(&mut self, chunk: &mc_world::Chunk, now: u64) {
        if !self.covered.insert([chunk.pos.x, chunk.pos.z]) {
            return;
        }
        self.checkpoint_dirty.insert([chunk.pos.x, chunk.pos.z]);
        self.dirty = true;
        for tick in chunk.scheduled_fluid_ticks() {
            self.schedule(
                tick.pos,
                &tick.fluid,
                now.saturating_add(tick.trigger_tick),
                tick.priority,
            );
        }
    }
    pub(crate) fn drain(
        &mut self,
        now: u64,
        world: &crate::world::ChunkWorld,
    ) -> Vec<FluidTickImage> {
        let selected: Vec<_> = self
            .due
            .iter()
            .take_while(|(order, _)| order.0 <= now)
            .filter(|(_, tick)| {
                world.is_block_active(tick.pos[0].div_euclid(16), tick.pos[2].div_euclid(16))
                    && world
                        .block_at(tick.pos[0], tick.pos[1], tick.pos[2])
                        .is_some()
            })
            .map(|(order, _)| *order)
            .collect();
        let mut ticks = Vec::with_capacity(selected.len());
        for order in selected {
            let tick = self.due.remove(&order).expect("selected tick");
            let (fluid, flowing) = identity(&tick.fluid).expect("validated fluid tick identifier");
            self.queued.remove(&(
                BlockPos {
                    x: tick.pos[0],
                    y: tick.pos[1],
                    z: tick.pos[2],
                },
                fluid,
                flowing,
                tick.deadline,
                tick.priority,
            ));
            self.checkpoint_dirty
                .insert([tick.pos[0].div_euclid(16), tick.pos[2].div_euclid(16)]);
            ticks.push(tick);
            self.dirty = true;
        }
        ticks
    }
    pub(crate) fn journal(&mut self, entries: &mut Vec<crate::journal::JournalEntry>) {
        if std::mem::take(&mut self.dirty) {
            entries.push(crate::journal::JournalEntry::FluidTicks {
                image: self.image(),
            });
        }
    }
    pub(crate) fn checkpoint(
        &self,
        world: &mut crate::world::ChunkWorld,
        now: u64,
    ) -> Vec<(ChunkPos, Vec<ScheduledFluidTick>)> {
        if self.checkpoint_dirty.is_empty() {
            return Vec::new();
        }
        // Project only the backlog: unchanged durable queues stay untouched
        // so a no-op save captures nothing. Pre-seeding from the backlog
        // (never from the live queue) keeps explicit empty clears while
        // admitting no new chunks. Retry-safe: clearing waits for the ACK.
        let mut chunks: BTreeMap<[i32; 2], Vec<ScheduledFluidTick>> = self
            .checkpoint_dirty
            .iter()
            .map(|pos| (*pos, Vec::new()))
            .collect();
        for tick in self.due.values() {
            if let Some(slot) = chunks.get_mut(&chunk_of_tick(tick)) {
                slot.push(ScheduledFluidTick::from_storage(
                    BlockPos {
                        x: tick.pos[0],
                        y: tick.pos[1],
                        z: tick.pos[2],
                    },
                    tick.fluid.clone(),
                    tick.deadline.saturating_sub(now),
                    tick.priority,
                    tick.sequence,
                ));
            }
        }
        let mut absent = Vec::new();
        for ([x, z], ticks) in chunks {
            let pos = ChunkPos { x, z };
            if world.has_chunk(x, z) {
                world.set_scheduled_fluid_ticks(pos, ticks);
            } else {
                absent.push((pos, ticks));
            }
        }
        absent
    }
    pub(crate) fn acknowledge_flushed(&mut self, image: &FluidTicksImage) {
        if self.checkpoint_dirty.is_empty() {
            return;
        }
        // Only matching captured queues release pending native projections.
        let pending = &self.checkpoint_dirty;
        let captured = grouped_by_chunk(
            image
                .due
                .iter()
                .filter(|tick| pending.contains(&chunk_of_tick(tick))),
        );
        let live = grouped_by_chunk(
            self.due
                .values()
                .filter(|tick| pending.contains(&chunk_of_tick(tick))),
        );
        for chunk in &image.covered {
            if !self.checkpoint_dirty.contains(chunk)
                || !rows_equal(rows(&live, chunk), rows(&captured, chunk))
            {
                continue;
            }
            self.checkpoint_dirty.remove(chunk);
            if rows(&live, chunk).is_empty() && self.covered.contains(chunk) {
                self.flushed_empty.insert(*chunk);
            }
        }
    }

    pub(crate) fn evict_flushed(&mut self, mut unloaded_clean: impl FnMut(i32, i32) -> bool) {
        if self.dirty {
            return;
        }
        let covered = &mut self.covered;
        let pending = &self.checkpoint_dirty;
        let mut removed = false;
        self.flushed_empty.retain(|&[x, z]| {
            if pending.contains(&[x, z]) || !unloaded_clean(x, z) {
                return true;
            }
            removed |= covered.remove(&[x, z]);
            false
        });
        // Fluid WAL is a whole-queue image; durably publish compacted coverage.
        self.dirty |= removed;
    }
}
