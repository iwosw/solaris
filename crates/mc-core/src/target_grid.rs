//! Per-tick nearest-target index for mob target search.
//!
//! Bottleneck: every thinking chaser scanned every player, villager and golem
//! target linearly, O(chasers x targets) per tick. Boundary: a grid is built
//! sequentially at most once per tick over an already ordered target table (one
//! grid per target class) and only for a class with enough consumers this tick
//! (otherwise the grid is the plain linear scan, no build cost); it borrows the
//! table, is never persisted and has no authority. Correctness fence:
//! [`TargetGrid::nearest`] returns exactly what the linear first-minimum scan
//! over the table returns (lowest table index among equal scores), provided
//! `score` is `None` for every target farther than `reach` blocks horizontally;
//! the equivalence tests pin this on random layouts. Removal path: replace
//! [`TargetGrid::nearest`] by its linear loop (identical result) and delete this
//! module.

use rustc_hash::FxHashMap as HashMap;

use crate::boundary::EntityId;

/// One target: entity id and feet position.
pub(crate) type Target = (EntityId, [f64; 3]);

/// Smaller tables are always scanned linearly; indexing them costs more than
/// it saves.
const MIN_INDEXED: usize = 32;

/// A class needs at least this many consumers in a tick before its index is
/// built: below that, the O(n) build costs about what the scans it saves cost.
pub(crate) const MIN_CONSUMERS: usize = 8;

pub(crate) struct TargetGrid<'a> {
    table: &'a [Target],
    /// Chunk -> (start, len) into `members`; empty when the table is scanned
    /// linearly.
    cells: HashMap<(i32, i32), (u32, u32)>,
    /// Table indices grouped by chunk, ascending inside a chunk.
    members: Vec<u32>,
}

fn cell(pos: [f64; 3]) -> (i32, i32) {
    (
        (pos[0].floor() as i32).div_euclid(16),
        (pos[2].floor() as i32).div_euclid(16),
    )
}

pub(crate) fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    let dx = b[0] - a[0];
    let dy = b[1] - a[1];
    let dz = b[2] - a[2];
    (dx * dx + dy * dy + dz * dz).sqrt()
}

impl<'a> TargetGrid<'a> {
    /// Plain linear scan of `table`; nothing is built.
    pub(crate) fn linear(table: &'a [Target]) -> Self {
        Self {
            table,
            cells: HashMap::default(),
            members: Vec::new(),
        }
    }

    /// Index for a class with `consumers` searches this tick.
    pub(crate) fn for_consumers(table: &'a [Target], consumers: usize) -> Self {
        if consumers >= MIN_CONSUMERS {
            Self::indexed(table)
        } else {
            Self::linear(table)
        }
    }

    /// Chunk buckets by counting sort: O(n), ascending table index per chunk.
    pub(crate) fn indexed(table: &'a [Target]) -> Self {
        if table.len() < MIN_INDEXED {
            return Self::linear(table);
        }
        let mut cells: HashMap<(i32, i32), (u32, u32)> = HashMap::default();
        let keys: Vec<(i32, i32)> = table
            .iter()
            .map(|(_, pos)| {
                let key = cell(*pos);
                cells.entry(key).or_insert((0, 0)).1 += 1;
                key
            })
            .collect();
        let mut offset = 0;
        for (start, len) in cells.values_mut() {
            *start = offset;
            offset += *len;
            // Reused as the fill cursor below, ending equal to the count.
            *len = 0;
        }
        let mut members = vec![0u32; table.len()];
        for (index, key) in keys.iter().enumerate() {
            let (start, filled) = cells.get_mut(key).expect("every key was counted");
            members[(*start + *filled) as usize] = index as u32;
            *filled += 1;
        }
        Self {
            table,
            cells,
            members,
        }
    }

    pub(crate) fn table(&self) -> &'a [Target] {
        self.table
    }

    /// Table index and score of the lowest-score target (lowest index among
    /// equal scores). `score` returns `None` to reject a target and MUST
    /// reject every target more than `reach` blocks away horizontally.
    pub(crate) fn nearest(
        &self,
        from: [f64; 3],
        reach: f64,
        mut score: impl FnMut(&Target) -> Option<f64>,
    ) -> Option<(usize, f64)> {
        let mut best: Option<(usize, f64)> = None;
        let radius = ((reach / 16.0).ceil() as i32).max(0);
        let side = 2 * i64::from(radius) + 1;
        if self.cells.is_empty() || side * side >= self.table.len() as i64 {
            for (index, target) in self.table.iter().enumerate() {
                if let Some(value) = score(target)
                    && best.is_none_or(|(_, lowest)| value < lowest)
                {
                    best = Some((index, value));
                }
            }
            return best;
        }
        let (cx, cz) = cell(from);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let Some(&(start, len)) = self.cells.get(&(cx + dx, cz + dz)) else {
                    continue;
                };
                for &member in &self.members[start as usize..(start + len) as usize] {
                    let index = member as usize;
                    if let Some(value) = score(&self.table[index])
                        && best.is_none_or(|(at, lowest)| {
                            value < lowest || (value == lowest && index < at)
                        })
                    {
                        best = Some((index, value));
                    }
                }
            }
        }
        best
    }

    /// Nearest accepted target within `range` blocks (3D distance).
    pub(crate) fn nearest_within(
        &self,
        from: [f64; 3],
        range: f64,
        accept: impl Fn(&Target) -> bool,
    ) -> Option<(usize, f64)> {
        self.nearest(from, range, |target| {
            let dist = distance(from, target.1);
            (dist <= range && accept(target)).then_some(dist)
        })
    }

    /// Whether any target satisfies a predicate bounded by horizontal `reach`.
    /// Unlike nearest-target selection, existence queries stop at the first hit.
    pub(crate) fn any(
        &self,
        from: [f64; 3],
        reach: f64,
        mut accepts: impl FnMut(&Target) -> bool,
    ) -> bool {
        let radius = ((reach / 16.0).ceil() as i32).max(0);
        let side = 2 * i64::from(radius) + 1;
        if self.cells.is_empty() || side * side >= self.table.len() as i64 {
            return self.table.iter().any(accepts);
        }
        let (cx, cz) = cell(from);
        for dz in -radius..=radius {
            for dx in -radius..=radius {
                let Some(&(start, len)) = self.cells.get(&(cx + dx, cz + dz)) else {
                    continue;
                };
                if self.members[start as usize..(start + len) as usize]
                    .iter()
                    .any(|&index| accepts(&self.table[index as usize]))
                {
                    return true;
                }
            }
        }
        false
    }
}

/// Running nearest across target classes visited in chain order.
pub(crate) type Nearest = Option<(Target, f64)>;

impl TargetGrid<'_> {
    /// Fold this class into `best`: a later class only replaces an earlier
    /// hit when strictly nearer, so ties keep chain order then table order,
    /// exactly like one linear strict-`<` scan over the concatenated tables.
    pub(crate) fn consider(
        &self,
        best: &mut Nearest,
        from: [f64; 3],
        range: f64,
        accept: impl Fn(&Target) -> bool,
    ) {
        if let Some((index, dist)) = self.nearest_within(from, range, accept)
            && best.is_none_or(|(_, lowest)| dist < lowest)
        {
            *best = Some((self.table[index], dist));
        }
    }
}
