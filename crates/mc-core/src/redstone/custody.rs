//! Mutation boundary for container signals. Read access cannot escape a mutable
//! map reference: every mutable inventory borrow, insertion/removal and viewer
//! transition records its position. Conservative mutable borrows are cheap;
//! signal recomputation still compares the resulting level before waking cells.

use std::collections::{BTreeMap, HashMap};
use std::ops::Deref;

use crate::boundary::{ChunkPos, EntityId};
use crate::tick::BlockEntityState;

type Cell = (i32, i32, i32);

#[derive(Debug, Default)]
struct DirtyPositions(Vec<Cell>);

impl DirtyPositions {
    fn mark_dirty(&mut self, cell: Cell) {
        self.0.push(cell);
    }

    fn drain_into(&mut self, cells: &mut Vec<Cell>) {
        cells.append(&mut self.0);
    }
}

/// Native container custody, including replay and chunk hydration. There is
/// deliberately no DerefMut/IndexMut or mutable iterator bypass.
#[derive(Debug, Default)]
pub struct BlockEntities {
    records: BTreeMap<Cell, BlockEntityState>,
    dirty: DirtyPositions,
    /// Journal consumer of the same custody marks (drained by the WAL writer).
    journal: Vec<Cell>,
    /// Independent 001-f consumer; hydration and catch-up never enter it.
    external: Vec<Cell>,
}

impl BlockEntities {
    pub fn get_mut(&mut self, cell: &Cell) -> Option<&mut BlockEntityState> {
        let record = self.records.get_mut(cell)?;
        self.dirty.mark_dirty(*cell);
        self.journal.push(*cell);
        self.external.push(*cell);
        Some(record)
    }

    pub fn insert(&mut self, cell: Cell, record: BlockEntityState) -> Option<BlockEntityState> {
        self.touch(cell);
        self.records.insert(cell, record)
    }

    pub fn remove(&mut self, cell: &Cell) -> Option<BlockEntityState> {
        let record = self.records.remove(cell)?;
        self.touch(*cell);
        Some(record)
    }

    pub fn entry(
        &mut self,
        cell: Cell,
    ) -> std::collections::btree_map::Entry<'_, Cell, BlockEntityState> {
        self.touch(cell);
        self.records.entry(cell)
    }

    /// Hydration must not acquire mutable live custody on an occupied entry.
    pub(crate) fn hydrate(&mut self, cell: Cell, record: impl FnOnce() -> BlockEntityState) {
        if let std::collections::btree_map::Entry::Vacant(entry) = self.records.entry(cell) {
            self.dirty.mark_dirty(cell);
            self.journal.push(cell);
            entry.insert(record());
        }
    }

    pub fn retain(&mut self, mut keep: impl FnMut(&Cell, &BlockEntityState) -> bool) {
        self.records.retain(|cell, record| {
            if keep(cell, record) {
                true
            } else {
                self.dirty.mark_dirty(*cell);
                self.external.push(*cell);
                self.journal.push(*cell);
                false
            }
        });
    }

    /// Non-slot comparator metadata (crafter disabled slots) shares this fence.
    pub(crate) fn mark_dirty(&mut self, cell: Cell) {
        self.touch(cell);
    }

    /// Residency also affects a double-chest half just across a chunk border.
    /// Only the transitioning column and its one-block halo are invalidated.
    pub(crate) fn mark_chunk_dirty(&mut self, chunk: ChunkPos) {
        let x = chunk.x.saturating_mul(16);
        let z = chunk.z.saturating_mul(16);
        let zs = z.saturating_sub(1)..=z.saturating_add(16);
        for (&cell, _) in self.records.range(
            (x.saturating_sub(1), i32::MIN, i32::MIN)..=(x.saturating_add(16), i32::MAX, i32::MAX),
        ) {
            if zs.contains(&cell.2) {
                self.dirty.mark_dirty(cell);
            }
        }
    }

    pub(crate) fn drain_dirty_into(&mut self, cells: &mut Vec<Cell>) {
        self.dirty.drain_into(cells);
    }

    /// Cells whose record may differ from its last journaled image, sorted and
    /// deduplicated. Every live record is either journaled or listed here.
    pub(crate) fn take_journal_touched(&mut self) -> Vec<Cell> {
        let mut cells = std::mem::take(&mut self.journal);
        cells.sort_unstable();
        cells.dedup();
        cells
    }

    pub(crate) fn take_external_writes(&mut self) -> Vec<Cell> {
        std::mem::take(&mut self.external)
    }

    pub(crate) fn get_mut_catch_up(&mut self, cell: &Cell) -> Option<&mut BlockEntityState> {
        let record = self.records.get_mut(cell)?;
        self.dirty.mark_dirty(*cell);
        self.journal.push(*cell);
        Some(record)
    }

    fn touch(&mut self, cell: Cell) {
        self.dirty.mark_dirty(cell);
        self.journal.push(cell);
        self.external.push(cell);
    }
}

impl Deref for BlockEntities {
    type Target = BTreeMap<Cell, BlockEntityState>;

    fn deref(&self) -> &Self::Target {
        &self.records
    }
}

impl<'a> IntoIterator for &'a BlockEntities {
    type Item = (&'a Cell, &'a BlockEntityState);
    type IntoIter = std::collections::btree_map::Iter<'a, Cell, BlockEntityState>;

    fn into_iter(self) -> Self::IntoIter {
        self.records.iter()
    }
}

/// Viewer transitions dirty both the previous and the new menu location.
#[derive(Debug, Default)]
pub struct OpenContainers {
    viewers: HashMap<EntityId, Cell>,
    dirty: DirtyPositions,
}

impl OpenContainers {
    pub fn insert(&mut self, viewer: EntityId, cell: Cell) -> Option<Cell> {
        let previous = self.viewers.insert(viewer, cell);
        if previous != Some(cell) {
            if let Some(previous) = previous {
                self.dirty.mark_dirty(previous);
            }
            self.dirty.mark_dirty(cell);
        }
        previous
    }

    pub fn remove(&mut self, viewer: &EntityId) -> Option<Cell> {
        let cell = self.viewers.remove(viewer)?;
        self.dirty.mark_dirty(cell);
        Some(cell)
    }

    pub fn clear(&mut self) {
        for (_, cell) in self.viewers.drain() {
            self.dirty.mark_dirty(cell);
        }
    }

    pub(crate) fn drain_dirty_into(&mut self, cells: &mut Vec<Cell>) {
        self.dirty.drain_into(cells);
    }
}

impl Deref for OpenContainers {
    type Target = HashMap<EntityId, Cell>;

    fn deref(&self) -> &Self::Target {
        &self.viewers
    }
}

impl<'a> IntoIterator for &'a OpenContainers {
    type Item = (&'a EntityId, &'a Cell);
    type IntoIter = std::collections::hash_map::Iter<'a, EntityId, Cell>;

    fn into_iter(self) -> Self::IntoIter {
        self.viewers.iter()
    }
}
