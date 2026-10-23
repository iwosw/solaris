//! Pure, bounded plans over the tick owner's retained world.
use super::blocks::{HORIZONTAL, NEIGHBOURS, offset};
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId};
use std::collections::{HashMap, HashSet, VecDeque};

#[derive(Clone, Copy)]
pub(super) struct BlockEdit {
    pub pos: BlockPos,
    pub new_state: BlockStateId,
}
pub(super) trait BlockPlanningRead {
    fn get_cached_block(&self, pos: BlockPos) -> Option<BlockStateId>;
    fn emission(&self, source: BlockPos, target: BlockPos) -> u8;
    fn protection_allows(&self, pos: BlockPos) -> bool;
}
pub(super) fn block_state_property<'a>(state: &'a BlockState, key: &str) -> Option<&'a str> {
    super::prop(state, key)
}
pub(super) fn sibling_state_with_bool_property(
    blocks: &BlockRegistry,
    state: &BlockState,
    key: &str,
    value: bool,
) -> Option<BlockStateId> {
    super::sibling(blocks, state, key, if value { "true" } else { "false" })
}
pub(super) fn air_state_id(blocks: &BlockRegistry) -> BlockStateId {
    blocks
        .block(&mc_data::Identifier::parse("minecraft:air").expect("static identifier"))
        .expect("air registry")
        .default
}
pub(super) struct Settle<'a, R: BlockPlanningRead + ?Sized> {
    pub blocks: &'a BlockRegistry,
    read: &'a R,
    overlay: HashMap<BlockPos, BlockStateId>,
    edits: Vec<BlockEdit>,
}
impl<'a, R: BlockPlanningRead + ?Sized> Settle<'a, R> {
    pub fn new(blocks: &'a BlockRegistry, read: &'a R) -> Self {
        Self {
            blocks,
            read,
            overlay: HashMap::new(),
            edits: Vec::new(),
        }
    }
    pub fn block_at(&self, pos: BlockPos) -> Option<BlockStateId> {
        self.overlay
            .get(&pos)
            .copied()
            .or_else(|| self.read.get_cached_block(pos))
    }
    pub fn state_at(&self, pos: BlockPos) -> Option<&'a BlockState> {
        self.blocks.by_id(self.block_at(pos)?)
    }
    pub fn protection_allows(&self, pos: BlockPos) -> bool {
        self.block_at(pos).is_some() && self.read.protection_allows(pos)
    }
    pub fn plan(&mut self, pos: BlockPos, new_state: BlockStateId) -> bool {
        if self.block_at(pos) == Some(new_state) {
            return false;
        }
        self.overlay.insert(pos, new_state);
        if let Some(edit) = self.edits.iter_mut().find(|edit| edit.pos == pos) {
            edit.new_state = new_state;
        } else {
            self.edits.push(BlockEdit { pos, new_state });
        }
        true
    }
    pub fn into_edits(self) -> Vec<BlockEdit> {
        self.edits
    }
    fn dust(&self, pos: BlockPos) -> bool {
        self.state_at(pos)
            .is_some_and(|state| state.block.id.path() == "redstone_wire")
    }
    fn conductor(&self, pos: BlockPos) -> bool {
        self.state_at(pos).is_some_and(super::conducts)
    }
    fn neighbours(&self, pos: BlockPos) -> impl Iterator<Item = BlockPos> {
        let mut out = [None; 8];
        for (index, (_, d)) in HORIZONTAL.into_iter().enumerate() {
            let Some(side) = offset(pos, d) else {
                continue;
            };
            if self.dust(side) {
                out[index * 2] = Some(side);
            }
            if self.conductor(side)
                && offset(pos, (0, 1, 0)).is_some_and(|above| !self.conductor(above))
            {
                if let Some(up) = offset(side, (0, 1, 0))
                    && self.dust(up)
                {
                    out[index * 2 + 1] = Some(up);
                }
            } else if !self.conductor(side)
                && let Some(down) = offset(side, (0, -1, 0))
                && self.dust(down)
            {
                out[index * 2 + 1] = Some(down);
            }
        }
        out.into_iter().flatten()
    }
    /// Return settled cells and a deferred frontier under the shared tick budget.
    pub fn dust_component(
        &mut self,
        start: BlockPos,
        limit: usize,
        settled: &HashSet<BlockPos>,
    ) -> (Vec<BlockPos>, Vec<BlockPos>) {
        let mut seen = HashSet::new();
        let mut queue = VecDeque::new();
        for d in std::iter::once((0, 0, 0)).chain(NEIGHBOURS) {
            if let Some(p) = offset(start, d)
                && self.dust(p)
                && !settled.contains(&p)
                && seen.insert(p)
            {
                queue.push_back(p);
            }
        }
        let mut cells = Vec::new();
        while cells.len() < limit {
            let Some(p) = queue.pop_front() else {
                break;
            };
            cells.push(p);
            for n in self.neighbours(p) {
                if !settled.contains(&n) && seen.insert(n) {
                    queue.push_back(n);
                }
            }
        }
        let mut levels: HashMap<BlockPos, u8> = cells.iter().map(|p| (*p, 0)).collect();
        for _ in 0..15 {
            let mut changed = false;
            for p in &cells {
                let mut best = 0;
                for d in NEIGHBOURS {
                    if let Some(n) = offset(*p, d)
                        && !self.dust(n)
                    {
                        best = best.max(self.read.emission(n, *p));
                    }
                }
                for n in self.neighbours(*p) {
                    best = best.max(
                        levels
                            .get(&n)
                            .copied()
                            .unwrap_or_else(|| {
                                self.state_at(n)
                                    .and_then(|s| super::prop(s, "power")?.parse().ok())
                                    .unwrap_or(0)
                            })
                            .saturating_sub(1),
                    );
                }
                if levels[p] != best {
                    levels.insert(*p, best);
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        for &p in &cells {
            let Some(state) = self.state_at(p) else {
                continue;
            };
            let mut props = state.properties.clone();
            for (key, value) in &mut props {
                if key == "power" {
                    *value = levels[&p].to_string();
                    continue;
                }
                if let Some((_, d)) = HORIZONTAL.iter().find(|(name, _)| name == key) {
                    let Some(n) = offset(p, *d) else {
                        continue;
                    };
                    let shape = if self.dust(n)
                        || self
                            .state_at(n)
                            .is_some_and(|s| super::signal::connects(s, *d))
                    {
                        "side"
                    } else if self.conductor(n)
                        && offset(p, (0, 1, 0)).is_some_and(|above| !self.conductor(above))
                        && offset(n, (0, 1, 0)).is_some_and(|up| self.dust(up))
                    {
                        "up"
                    } else if !self.conductor(n)
                        && offset(n, (0, -1, 0)).is_some_and(|down| self.dust(down))
                    {
                        "side"
                    } else {
                        "none"
                    };
                    *value = shape.into();
                }
            }
            let mut connected = [false; 4];
            for (index, (name, _)) in HORIZONTAL.iter().enumerate() {
                connected[index] = props
                    .iter()
                    .any(|(key, value)| key == name && value != "none");
            }
            let count = connected.iter().filter(|connected| **connected).count();
            if count <= 1 {
                for (index, (name, _)) in HORIZONTAL.iter().enumerate() {
                    if (count == 0 || connected[index ^ 1])
                        && let Some((_, value)) = props.iter_mut().find(|(key, _)| key == name)
                    {
                        *value = "side".into();
                    }
                }
            }
            if let Some(new) = self.blocks.by_name_and_props(&state.block.id, &props) {
                self.plan(p, new);
            }
        }
        (cells, queue.into_iter().collect())
    }
}
