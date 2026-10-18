//! Persistent wire images in dense copy-on-write pages. Identity and spatial
//! membership change only on lifecycle/chunk moves; a publication builds each
//! touched page once, directly from the new after-images, and leaves every
//! previously published page untouched.
use crate::boundary::{ChunkPos, EntityId, EntityView};
use im::{OrdMap, OrdSet};
use rayon::prelude::*;
use std::sync::Arc;

type Page = Vec<Option<EntityView>>;
type ChunkKey = (i32, i32);

#[derive(Debug, Clone, Default)]
pub struct EntityViews {
    // Ordinary motion never rewrites the identity index. Retained snapshots
    // share it, while page COW bounds wire-image copying to changed pages.
    rows: Arc<rustc_hash::FxHashMap<EntityId, usize>>,
    pages: Arc<Vec<Arc<Page>>>,
    free: Arc<Vec<usize>>,
    chunks: OrdMap<ChunkKey, OrdSet<EntityId>>,
    revision: u64,
    base_revision: u64,
    changed: Arc<Vec<EntityId>>,
}

impl EntityViews {
    pub fn len(&self) -> usize {
        self.rows.len()
    }
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn get(&self, id: EntityId) -> Option<&EntityView> {
        let slot = *self.rows.get(&id)?;
        self.pages[slot / ROWS_PER_PAGE][slot % ROWS_PER_PAGE].as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn base_revision(&self) -> u64 {
        self.base_revision
    }
    /// Actual wire after-image changes, including IDs removed in this version.
    pub fn changed(&self) -> &[EntityId] {
        &self.changed
    }
    /// Preserve the publication order: signed chunk coordinates, then wire ID.
    pub fn iter(&self) -> EntityViewIter<'_> {
        EntityViewIter {
            chunks: self.chunks.iter(),
            current: None,
            rows: self,
            remaining: self.rows.len(),
        }
    }
    pub fn in_chunk(&self, pos: ChunkPos) -> impl Iterator<Item = &EntityView> {
        self.chunks
            .get(&(pos.x, pos.z))
            .into_iter()
            .flat_map(|ids| ids.iter())
            .map(|id| self.get(*id).expect("indexed wire entity"))
    }

    /// Publish one version from candidate identities in strictly ascending ID
    /// order. `resolve` names the candidate's ID and its source (`None` = the
    /// entity is gone); `project` builds the after-image from a source. Both run
    /// on the rayon pool for large batches, so they must be pure reads.
    ///
    /// Wire slots and the identity index change serially in ID order (the same
    /// assignment as one-at-a-time replacement). Every touched page is then
    /// built once: changed cells take the new after-image by move, the others
    /// are cloned from the previous page. Pages of earlier versions are never
    /// mutated, and nothing is visible to `self` readers until the batch is
    /// complete. The version/revision advances only if an image actually changed.
    pub(crate) fn publish<'c, C, S>(
        &mut self,
        candidates: &'c [C],
        resolve: impl Fn(&'c C) -> (EntityId, Option<S>) + Sync,
        project: impl Fn(S) -> EntityView + Sync,
    ) where
        C: Sync,
        S: Copy + Send + Sync,
    {
        if candidates.is_empty() {
            return;
        }
        let rows = &*self.rows;
        let lookup = |candidate: &'c C| {
            let (id, src) = resolve(candidate);
            Resolved {
                id,
                src,
                slot: rows.get(&id).copied(),
            }
        };
        let mut resolved: Vec<Resolved<S>> = Vec::new();
        if candidates.len() >= PARALLEL_RESOLVE_MIN {
            candidates
                .par_iter()
                .with_min_len(1024)
                .map(&lookup)
                .collect_into_vec(&mut resolved);
        } else {
            resolved.extend(candidates.iter().map(&lookup));
        }
        debug_assert!(resolved.windows(2).all(|pair| pair[0].id < pair[1].id));
        assert!(u32::try_from(resolved.len()).is_ok());

        // Serial, ID-ordered lifecycle: slot reuse and appends are deterministic.
        let mut next_append = self.pages.last().map_or(0, |last| {
            (self.pages.len() - 1) * ROWS_PER_PAGE + last.len()
        });
        let mut entries = Vec::with_capacity(resolved.len());
        for (index, row) in resolved.iter().enumerate() {
            let index = index as u32;
            match (row.slot, row.src.is_some()) {
                (Some(slot), true) => entries.push(Entry {
                    slot,
                    index,
                    clear: false,
                }),
                (Some(slot), false) => {
                    entries.push(Entry {
                        slot,
                        index,
                        clear: true,
                    });
                    Arc::make_mut(&mut self.rows).remove(&row.id);
                    Arc::make_mut(&mut self.free).push(slot);
                }
                (None, true) => {
                    let slot = if self.free.is_empty() {
                        next_append += 1;
                        next_append - 1
                    } else {
                        Arc::make_mut(&mut self.free)
                            .pop()
                            .expect("non-empty free list")
                    };
                    Arc::make_mut(&mut self.rows).insert(row.id, slot);
                    entries.push(Entry {
                        slot,
                        index,
                        clear: false,
                    });
                }
                (None, false) => {}
            }
        }
        // Stable: a slot freed and reused in this batch keeps clear-then-set order.
        entries.sort_by_key(|entry| entry.slot);

        let mut groups: Vec<(usize, &[Entry])> = Vec::new();
        let mut start = 0;
        while start < entries.len() {
            let page = entries[start].slot / ROWS_PER_PAGE;
            let mut end = start + 1;
            while end < entries.len() && entries[end].slot / ROWS_PER_PAGE == page {
                end += 1;
            }
            groups.push((page, &entries[start..end]));
            start = end;
        }
        let pages = &*self.pages;
        let build = |&(page, group): &(usize, &[Entry])| {
            build_page(
                pages.get(page).map(|old| old.as_slice()),
                page,
                group,
                &resolved,
                &project,
            )
        };
        let built: Vec<PageOut> = if entries.len() >= PARALLEL_PAGE_MIN_ENTRIES
            && groups.len() >= PARALLEL_PAGE_MIN_GROUPS
        {
            groups.par_iter().with_min_len(4).map(&build).collect()
        } else {
            groups.iter().map(&build).collect()
        };

        if built.iter().all(|out| out.next.is_none()) {
            return;
        }
        let mut changed = Vec::with_capacity(built.iter().map(|out| out.changed.len()).sum());
        let pages = Arc::make_mut(&mut self.pages);
        for out in built {
            let Some(page) = out.next else { continue };
            if out.page == pages.len() {
                pages.push(page);
            } else {
                pages[out.page] = page;
            }
            changed.extend(out.changed);
            for step in out.moves {
                apply_move(&mut self.chunks, step);
            }
        }
        self.finish(changed);
    }

    fn finish(&mut self, mut changed: Vec<EntityId>) {
        static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        if changed.len() >= PARALLEL_SORT_MIN {
            changed.par_sort_unstable();
        } else {
            changed.sort_unstable();
        }
        self.base_revision = self.revision;
        self.revision = REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        self.changed = Arc::new(changed);
    }
}

struct Resolved<S> {
    id: EntityId,
    src: Option<S>,
    slot: Option<usize>,
}

/// One cell write: set from `resolved[index].src`, or clear the cell of the
/// entity `resolved[index].id` that left.
struct Entry {
    slot: usize,
    index: u32,
    clear: bool,
}

struct ChunkMove {
    id: EntityId,
    before: Option<ChunkKey>,
    after: Option<ChunkKey>,
}

struct PageOut {
    page: usize,
    /// `None` when every entry reproduced the previous image: the old page stays shared.
    next: Option<Arc<Page>>,
    changed: Vec<EntityId>,
    moves: Vec<ChunkMove>,
}

fn build_page<S: Copy>(
    old: Option<&[Option<EntityView>]>,
    page: usize,
    group: &[Entry],
    resolved: &[Resolved<S>],
    project: &impl Fn(S) -> EntityView,
) -> PageOut {
    let base = page * ROWS_PER_PAGE;
    let old_len = old.map_or(0, <[_]>::len);
    let mut changed = Vec::with_capacity(group.len());
    let mut moves = Vec::new();
    // Built on the first effective change; an unchanged page stays shared.
    // `copied` old cells are already in `cells` (cloned or replaced).
    let mut cells: Option<Vec<Option<EntityView>>> = None;
    let mut copied = 0;
    // `group` is stably sorted by slot: entries of one cell are adjacent and
    // keep their clear-then-set order.
    let mut start = 0;
    while start < group.len() {
        let slot = group[start].slot;
        let mut end = start + 1;
        while end < group.len() && group[end].slot == slot {
            end += 1;
        }
        let cell = slot - base;
        let mut staged: Option<Option<EntityView>> = None;
        for entry in &group[start..end] {
            let row = &resolved[entry.index as usize];
            let current = match &staged {
                Some(staged) => staged.as_ref(),
                None => old.and_then(|old| old.get(cell)?.as_ref()),
            };
            if entry.clear {
                debug_assert!(current.is_some());
                let before = current.map(|view| chunk(view.position));
                staged = Some(None);
                changed.push(row.id);
                if before.is_some() {
                    moves.push(ChunkMove {
                        id: row.id,
                        before,
                        after: None,
                    });
                }
            } else {
                let next = project(row.src.expect("set entry has a source"));
                if current.is_some_and(|current| view_eq(current, &next)) {
                    continue;
                }
                let before = current.map(|view| chunk(view.position));
                let after = chunk(next.position);
                staged = Some(Some(next));
                changed.push(row.id);
                if before != Some(after) {
                    moves.push(ChunkMove {
                        id: row.id,
                        before,
                        after: Some(after),
                    });
                }
            }
        }
        if let Some(image) = staged {
            let cells = cells.get_or_insert_with(|| {
                // Final length: appends are contiguous, so the last entry's cell bounds it.
                Vec::with_capacity(old_len.max(group[group.len() - 1].slot - base + 1))
            });
            if copied < cell {
                cells.extend_from_slice(
                    &old.expect("unassigned cell has a previous page")[copied..cell],
                );
            }
            cells.push(image);
            copied = cell + 1;
        }
        start = end;
    }
    let next = cells.map(|mut cells| {
        if copied < old_len {
            cells.extend_from_slice(&old.expect("old cells")[copied..]);
        }
        Arc::new(cells)
    });
    PageOut {
        page,
        next,
        changed,
        moves,
    }
}

fn apply_move(chunks: &mut OrdMap<ChunkKey, OrdSet<EntityId>>, step: ChunkMove) {
    if let Some(chunk) = step.before {
        let ids = chunks.get_mut(&chunk).expect("indexed wire chunk");
        ids.remove(&step.id);
        if ids.is_empty() {
            chunks.remove(&chunk);
        }
    }
    if let Some(chunk) = step.after {
        chunks.entry(chunk).or_default().insert(step.id);
    }
}

/// Field-wise equality with an identity fast path for the shared appearance.
fn view_eq(a: &EntityView, b: &EntityView) -> bool {
    let EntityView {
        id,
        uuid,
        type_id,
        position,
        velocity,
        yaw,
        pitch,
        head_yaw,
        on_ground,
        appearance,
    } = a;
    *id == b.id
        && *uuid == b.uuid
        && *type_id == b.type_id
        && *position == b.position
        && *velocity == b.velocity
        && *yaw == b.yaw
        && *pitch == b.pitch
        && *head_yaw == b.head_yaw
        && *on_ground == b.on_ground
        && (Arc::ptr_eq(appearance, &b.appearance) || **appearance == *b.appearance)
}

fn chunk(position: [f64; 3]) -> ChunkKey {
    (
        mc_physics::mth::floor_i32(position[0]).div_euclid(16),
        mc_physics::mth::floor_i32(position[2]).div_euclid(16),
    )
}
impl From<Vec<EntityView>> for EntityViews {
    fn from(mut rows: Vec<EntityView>) -> Self {
        // The last duplicate identity wins.
        rows.reverse();
        rows.sort_by_key(|row| row.id);
        rows.dedup_by_key(|row| row.id);
        let mut views = Self::default();
        views.publish(&rows, |row| (row.id, Some(row)), EntityView::clone);
        views
    }
}

pub struct EntityViewIter<'a> {
    chunks: im::ordmap::Iter<'a, (i32, i32), OrdSet<EntityId>>,
    current: Option<im::ordset::Iter<'a, EntityId>>,
    rows: &'a EntityViews,
    remaining: usize,
}
impl<'a> Iterator for EntityViewIter<'a> {
    type Item = &'a EntityView;
    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(id) = self.current.as_mut().and_then(Iterator::next) {
                self.remaining -= 1;
                return Some(self.rows.get(*id).expect("indexed wire entity"));
            }
            let (_, ids) = self.chunks.next()?;
            self.current = Some(ids.iter());
        }
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.remaining, Some(self.remaining))
    }
}
impl ExactSizeIterator for EntityViewIter<'_> {}
impl<'a> IntoIterator for &'a EntityViews {
    type Item = &'a EntityView;
    type IntoIter = EntityViewIter<'a>;
    fn into_iter(self) -> Self::IntoIter {
        self.iter()
    }
}

const ROWS_PER_PAGE: usize = 64;
/// Below these sizes a rayon hand-off costs more than the serial loop.
const PARALLEL_RESOLVE_MIN: usize = 4096;
const PARALLEL_PAGE_MIN_ENTRIES: usize = 1024;
const PARALLEL_PAGE_MIN_GROUPS: usize = 16;
const PARALLEL_SORT_MIN: usize = 8192;

#[cfg(test)]
#[path = "entity_views_tests.rs"]
mod tests;
