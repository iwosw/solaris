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

fn connection_kind(path: &str) -> Option<u8> {
    if path.ends_with("_fence") {
        Some(0)
    } else if path.ends_with("_wall") {
        Some(1)
    } else if path.ends_with("_pane") || path == "glass_pane" || path == "iron_bars" {
        Some(2)
    } else {
        None
    }
}

fn connected_state(
    blocks: &BlockRegistry,
    pos: BlockPos,
    id: u32,
    kind: u8,
    read: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<u32> {
    use mc_data::block_facts::{SturdyFace, has_full_sturdy_face};
    let entry = blocks.by_id(BlockStateId(id))?;
    let mut props = entry.properties.clone();
    let mut connected = [false; 4];
    let directions = [Face::North, Face::South, Face::West, Face::East];
    let above = if kind == 1 {
        let id = read(step(pos, Face::Up))?;
        Some((id, blocks.by_id(BlockStateId(id))?))
    } else {
        None
    };
    let covered = above.is_some_and(|(id, block)| {
        has_full_sturdy_face(id, &block.block.id, &block.properties, SturdyFace::Down)
    });
    for (index, direction) in directions.into_iter().enumerate() {
        let neighbor_id = read(step(pos, direction))?;
        let neighbor = blocks.by_id(BlockStateId(neighbor_id))?;
        let neighbor_path = neighbor.block.id.path();
        let same = match kind {
            0 => {
                connection_kind(neighbor_path) == Some(0)
                    && (entry.block.id.path() == "nether_brick_fence")
                        == (neighbor_path == "nether_brick_fence")
            }
            1 => connection_kind(neighbor_path) == Some(1),
            _ => connection_kind(neighbor_path) == Some(2),
        };
        let gate = kind < 2
            && neighbor_path.ends_with("_fence_gate")
            && neighbor
                .properties
                .iter()
                .find(|(key, _)| key == "facing")
                .is_some_and(|(_, value)| {
                    if direction.axis() == "x" {
                        matches!(value.as_str(), "north" | "south")
                    } else {
                        matches!(value.as_str(), "east" | "west")
                    }
                });
        let face = match direction {
            Face::North => SturdyFace::South,
            Face::South => SturdyFace::North,
            Face::West => SturdyFace::East,
            _ => SturdyFace::West,
        };
        connected[index] = same
            || gate
            || has_full_sturdy_face(neighbor_id, &neighbor.block.id, &neighbor.properties, face);
        set(
            &mut props,
            direction.name(),
            if kind == 1 {
                if !connected[index] {
                    "none"
                } else if covered {
                    "tall"
                } else {
                    "low"
                }
            } else if connected[index] {
                "true"
            } else {
                "false"
            },
        );
    }
    if kind == 1 {
        let straight =
            connected == [true, true, false, false] || connected == [false, false, true, true];
        let post_above = above.is_some_and(|(_, block)| {
            block.block.id.path().ends_with("_wall")
                && block
                    .properties
                    .iter()
                    .any(|(key, value)| key == "up" && value == "true")
        });
        set(
            &mut props,
            "up",
            if !straight || covered || post_above {
                "true"
            } else {
                "false"
            },
        );
    }
    blocks
        .by_name_and_props(&entry.block.id, &props)
        .map(|state| state.0)
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
                step(*pos, Face::Down),
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
        if let Some(kind) = blocks
            .by_id(BlockStateId(id))
            .and_then(|entry| connection_kind(entry.block.id.path()))
        {
            let new = connected_state(blocks, pos, id, kind, &read)?;
            if new != id {
                updates.push((pos, new));
            }
            continue;
        }
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
        // The four perimeter strips and lower wall candidates reach the world.
        // In-batch cells and overlapping candidates are read from the index.
        assert_eq!(reads.get(), 64 + 256);
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
