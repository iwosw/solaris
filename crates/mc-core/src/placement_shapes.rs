use super::{BlockPos, Face, set, step};
use mc_data::block_placement_26_1_2::{
    StairCell, StairNeighborState, StairProperties, resolve_stair_shape,
};
use mc_domain::Direction;
use mc_world::{BlockRegistry, BlockStateId};
use rustc_hash::{FxHashMap, FxHashSet};

fn cell(blocks: &BlockRegistry, id: u32) -> StairCell {
    let Some(state) = blocks.by_id(BlockStateId(id)) else {
        return StairCell::Malformed;
    };
    if !state.block.id.path().ends_with("_stairs") {
        return StairCell::NotStair;
    }
    let prop = |key: &str| {
        state
            .properties
            .iter()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.as_str())
    };
    let facing = match prop("facing") {
        Some("north") => Direction::North,
        Some("south") => Direction::South,
        Some("west") => Direction::West,
        Some("east") => Direction::East,
        _ => return StairCell::Malformed,
    };
    let top = match prop("half") {
        Some("top") => true,
        Some("bottom") => false,
        _ => return StairCell::Malformed,
    };
    StairCell::Stair(StairProperties { facing, top })
}
/// Recompute affected corners from the complete after-image. Reads are
/// collected by the caller into the same intent footprint as occupancy.
///
/// Large command batches formerly searched the entire write list for every
/// neighbor (26.85% of tick-thread samples in the 2026-10-15 fill profile).
/// Index only batches larger than one placement, retaining last-write reads,
/// first-write replacement and first-seen neighbor order. All updates remain
/// staged until every required read succeeds; callers need no defensive clone.
pub(crate) fn shape_updates(
    blocks: &BlockRegistry,
    writes: &mut Vec<(BlockPos, u32)>,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<()> {
    let indexed = (writes.len() > 8).then(|| {
        let mut index = FxHashMap::default();
        index.reserve(writes.len());
        for (offset, &(pos, id)) in writes.iter().enumerate() {
            index
                .entry(pos)
                .and_modify(|entry: &mut (usize, u32)| entry.1 = id)
                .or_insert((offset, id));
        }
        index
    });
    let mut positions: Vec<_> = writes
        .iter()
        .flat_map(|(pos, _)| {
            [
                *pos,
                step(*pos, Face::North),
                step(*pos, Face::South),
                step(*pos, Face::West),
                step(*pos, Face::East),
            ]
        })
        .collect();
    if indexed.is_some() {
        let mut seen = FxHashSet::default();
        seen.reserve(positions.len());
        positions.retain(|pos| seen.insert(*pos));
    }
    let mut updates = Vec::new();
    for pos in positions {
        let read = |pos| {
            let pending = if let Some(index) = &indexed {
                index.get(&pos).map(|(_, id)| *id)
            } else {
                writes
                    .iter()
                    .rev()
                    .find(|(cell, _)| *cell == pos)
                    .map(|(_, id)| *id)
            };
            pending.or_else(|| block_at(pos))
        };
        let Some(id) = read(pos) else {
            continue;
        };
        let StairCell::Stair(current) = cell(blocks, id) else {
            continue;
        };
        let neighbors = StairNeighborState {
            north: cell(blocks, read(step(pos, Face::North))?),
            south: cell(blocks, read(step(pos, Face::South))?),
            west: cell(blocks, read(step(pos, Face::West))?),
            east: cell(blocks, read(step(pos, Face::East))?),
        };
        let shape = resolve_stair_shape(current, neighbors)?;
        let entry = blocks.by_id(BlockStateId(id))?;
        if entry
            .properties
            .iter()
            .any(|(key, value)| key == "shape" && value == shape.property_value())
        {
            continue;
        }
        let mut props = entry.properties.clone();
        set(&mut props, "shape", shape.property_value());
        let new = blocks.by_name_and_props(&entry.block.id, &props)?.0;
        if new != id {
            updates.push((pos, new));
        }
    }
    for (pos, id) in updates {
        if let Some(index) = &indexed {
            if let Some((offset, _)) = index.get(&pos) {
                writes[*offset].1 = id;
            } else {
                // Large-batch candidates were deduplicated before resolution.
                writes.push((pos, id));
            }
        } else if let Some((_, current)) = writes.iter_mut().find(|(cell, _)| *cell == pos) {
            *current = id;
        } else {
            writes.push((pos, id));
        }
    }
    Some(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::CoreRegistry;
    use std::cell::Cell;

    #[test]
    fn indexed_batch_matches_small_placement_after_image() {
        let registry = CoreRegistry::load(None).expect("registry");
        let blocks = &registry.blocks;
        let stair = blocks
            .block(&mc_data::Identifier::parse("oak_stairs").unwrap())
            .unwrap();
        let default = blocks.by_id(stair.default).unwrap();
        let state = |facing| {
            let mut props = default.properties.clone();
            set(&mut props, "facing", facing);
            set(&mut props, "half", "bottom");
            set(&mut props, "shape", "straight");
            blocks.by_name_and_props(&stair.id, &props).unwrap().0
        };
        let centre = BlockPos { x: 0, y: 1, z: 0 };
        let north = step(centre, Face::North);
        // Duplicate writes must retain the same last-read/first-replace rule.
        let original = vec![
            (centre, registry.states.stone),
            (centre, state("north")),
            (north, state("west")),
        ];
        let mut small = original.clone();
        let mut large = original;
        large.extend((0..64).map(|x| {
            (
                BlockPos {
                    x: 100 + x,
                    y: 1,
                    z: 100,
                },
                registry.states.stone,
            )
        }));
        let read = |_| Some(registry.states.air);
        assert_eq!(shape_updates(blocks, &mut small, &read), Some(()));
        assert_eq!(shape_updates(blocks, &mut large, &read), Some(()));
        large.retain(|(pos, _)| pos.x < 100);
        assert_eq!(large, small);
        assert_ne!(small[0].1, registry.states.stone, "corner must change");
    }

    #[test]
    fn indexed_batch_reads_each_distinct_neighbor_once() {
        let registry = CoreRegistry::load(None).expect("registry");
        let mut writes: Vec<_> = (0..16)
            .flat_map(|x| (0..16).map(move |z| (BlockPos { x, y: 1, z }, registry.states.stone)))
            .collect();
        let reads = Cell::new(0usize);
        let read = |_| {
            reads.set(reads.get() + 1);
            Some(registry.states.air)
        };
        assert_eq!(
            shape_updates(&registry.blocks, &mut writes, &read),
            Some(())
        );
        assert_eq!(writes.len(), 256);
        // All in-batch cells come from the index, only the four perimeter
        // strips reach the world. Internal overlaps must not be rescanned.
        assert_eq!(reads.get(), 64);
    }

    #[test]
    fn missing_required_neighbor_leaves_writes_untouched() {
        let registry = CoreRegistry::load(None).expect("registry");
        let blocks = &registry.blocks;
        let stair = blocks
            .block(&mc_data::Identifier::parse("oak_stairs").unwrap())
            .unwrap()
            .default
            .0;
        for count in [1, 16] {
            let mut writes: Vec<_> = (0..count)
                .map(|x| {
                    (
                        BlockPos {
                            x: x * 4,
                            y: 1,
                            z: 0,
                        },
                        stair,
                    )
                })
                .collect();
            let original = writes.clone();
            assert_eq!(shape_updates(blocks, &mut writes, &|_| None), None);
            assert_eq!(writes, original);
        }
    }
}
