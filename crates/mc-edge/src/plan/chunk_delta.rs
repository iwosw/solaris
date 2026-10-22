//! Publish-wide chunk revision delta shared by every viewer.
//!
//! A viewer that was fully reconciled against the previous publish and has no
//! outstanding delivery debt only needs the chunks that changed since. The
//! log diffs consecutive resident-chunk lists once per publish (one merge walk
//! over two sorted lists, none when the list is shared) instead of every
//! viewer re-probing every held chunk. Anything unusual - a skipped publish, a
//! moved window, a respawn, an overflowing list - falls back to the full
//! viewer walk, which stays the authority.
use super::*;
use mc_core::dimensions::Dimension;

/// Beyond this many changed chunks per publish the list is dropped: a viewer
/// window then costs about as much to walk as to look up, and the full walk
/// needs no list.
const MAX_CHUNK_CHANGES: usize = 256;

static NEXT_SOURCE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChunkChangeKind {
    /// Newly resident (first load or reload).
    Loaded,
    /// Still resident with another content/light revision.
    Revised,
    /// No longer resident.
    Unloaded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkChange {
    pub(crate) pos: (i32, i32),
    /// Revision after the change; zero when unloaded.
    pub(crate) revision: u64,
    pub(crate) kind: ChunkChangeKind,
}

/// Changes of one dimension since the previous publish of the same log.
#[derive(Debug, Default)]
pub(crate) struct ChunkDelta {
    source: u64,
    seq: u64,
    /// Publish sequence the diff is relative to; `None` without a usable base.
    base: Option<u64>,
    overflow: bool,
    changes: Vec<ChunkChange>,
}

impl ChunkDelta {
    pub(crate) fn changes(&self) -> &[ChunkChange] {
        &self.changes
    }

    /// True when a viewer synced at `sync` may apply [`Self::changes`] alone.
    pub(super) fn follows(&self, sync: &ChunkSync, dimension: Dimension) -> bool {
        !self.overflow
            && sync.dimension == dimension
            && sync.source == self.source
            && self.base == Some(sync.seq)
    }

    pub(super) fn sync(&self, dimension: Dimension, held: usize, quiet: bool) -> ChunkSync {
        ChunkSync {
            source: self.source,
            seq: self.seq,
            dimension,
            held,
            quiet,
        }
    }
}

/// What a viewer's chunk state was last proven against.
#[derive(Debug, Clone, Copy)]
pub(super) struct ChunkSync {
    source: u64,
    seq: u64,
    dimension: Dimension,
    /// `loaded_chunks.len()` when the plan ended; a different size means
    /// someone else changed the held set since.
    pub(super) held: usize,
    /// No repair, pending block, light, admission or unload debt remained and
    /// every held chunk in the window carried its resident revision.
    pub(super) quiet: bool,
}

#[derive(Debug)]
pub(crate) struct ChunkDeltaLog {
    source: u64,
    seq: u64,
    enabled: bool,
    previous: HashMap<Dimension, (u64, Arc<Vec<SnapshotChunk>>)>,
}

impl Default for ChunkDeltaLog {
    fn default() -> Self {
        Self {
            source: NEXT_SOURCE.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            seq: 0,
            enabled: true,
            previous: HashMap::default(),
        }
    }
}

impl ChunkDeltaLog {
    /// Reference behaviour: no delta, every viewer walks its whole window.
    #[cfg(test)]
    pub(crate) fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::default()
        }
    }

    /// Start the next publish; call once before [`Self::delta`].
    pub(crate) fn begin(&mut self) {
        self.seq += 1;
    }

    /// Diff `projection`'s resident list against this dimension's previous publish.
    pub(crate) fn delta(&mut self, projection: &TickOutput) -> Option<ChunkDelta> {
        if !self.enabled {
            return None;
        }
        let dimension = projection.snapshot.dimension;
        let chunks = &projection.snapshot.chunks;
        let previous = self
            .previous
            .insert(dimension, (self.seq, Arc::clone(chunks)));
        let mut delta = ChunkDelta {
            source: self.source,
            seq: self.seq,
            ..ChunkDelta::default()
        };
        let Some((at, old)) = previous.filter(|(at, _)| at + 1 == self.seq) else {
            return Some(delta);
        };
        delta.base = Some(at);
        if Arc::ptr_eq(&old, chunks) {
            return Some(delta);
        }
        let (mut i, mut j) = (0, 0);
        while i < old.len() || j < chunks.len() {
            if delta.changes.len() == MAX_CHUNK_CHANGES {
                delta.overflow = true;
                delta.changes.clear();
                break;
            }
            let (kind, pos, revision) = match (old.get(i), chunks.get(j)) {
                (Some(was), Some(now)) => {
                    let (was_pos, now_pos) = ((was.pos.x, was.pos.z), (now.pos.x, now.pos.z));
                    match was_pos.cmp(&now_pos) {
                        std::cmp::Ordering::Less => {
                            i += 1;
                            (ChunkChangeKind::Unloaded, was_pos, 0)
                        }
                        std::cmp::Ordering::Greater => {
                            j += 1;
                            (ChunkChangeKind::Loaded, now_pos, now.revision)
                        }
                        std::cmp::Ordering::Equal => {
                            i += 1;
                            j += 1;
                            if was.revision == now.revision {
                                continue;
                            }
                            (ChunkChangeKind::Revised, now_pos, now.revision)
                        }
                    }
                }
                (Some(was), None) => {
                    i += 1;
                    (ChunkChangeKind::Unloaded, (was.pos.x, was.pos.z), 0)
                }
                (None, Some(now)) => {
                    j += 1;
                    (
                        ChunkChangeKind::Loaded,
                        (now.pos.x, now.pos.z),
                        now.revision,
                    )
                }
                (None, None) => break,
            };
            delta.changes.push(ChunkChange {
                pos,
                revision,
                kind,
            });
        }
        Some(delta)
    }
}

/// Counters of the chunk-reconcile passes (test builds only).
#[cfg(test)]
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ChunkPlanStats {
    pub(crate) plans: u64,
    pub(crate) full_walks: u64,
    pub(crate) delta_plans: u64,
    /// Chunk entries iterated by reconcile passes.
    pub(crate) visited: u64,
    /// Keyed lookups: held/want/repair membership, resident binary searches,
    /// index and light-baseline probes.
    pub(crate) probes: u64,
}

#[cfg(test)]
impl ChunkPlanStats {
    pub(crate) fn add(&mut self, other: Self) {
        self.plans += other.plans;
        self.full_walks += other.full_walks;
        self.delta_plans += other.delta_plans;
        self.visited += other.visited;
        self.probes += other.probes;
    }
}

/// Counts in test builds only; compiled out of the server.
macro_rules! chunk_stat {
    ($stats:ident . $field:ident += $n:expr) => {
        #[cfg(test)]
        {
            $stats.$field += ($n) as u64;
        }
    };
}
pub(super) use chunk_stat;
