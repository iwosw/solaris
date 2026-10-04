//! A 5×5, 16 m/cell compact reference basin. DEM and runoff are explicit
//! causal inputs, not independently sampled drainage noise. A priority flood
//! fixes one boundary outlet, one spill lake, a complete drainage DAG and a
//! conservative discharge budget before any blocks are materialized.

use std::cmp::Reverse;
use std::collections::BinaryHeap;

use mc_data::Identifier;
use mc_world::{BlockRegistry, Chunk, ChunkPos, MIN_Y};

pub const BASIN_AXIS: usize = 5;
pub const BASIN_CELLS: usize = BASIN_AXIS * BASIN_AXIS;
pub const BASIN_CELL_METERS: i32 = 16;
pub const BASIN_OUTLET: usize = 22;
const DEM: [i32; BASIN_CELLS] = [
    120, 118, 116, 114, 112, 118, 100, 98, 96, 110, 116, 98, 82, 94, 108, 114, 96, 92, 90, 106,
    112, 110, 88, 104, 102,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BasinCell {
    pub ground_y: i32,
    /// Hydrological spill elevation, independent of sea level.
    pub spill_y: i32,
    pub downstream: Option<usize>,
    /// Fixed-point unit runoff per cell; outgoing flux includes all upstream.
    pub discharge: u32,
    pub flood_rank: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BasinFixture {
    cells: [BasinCell; BASIN_CELLS],
}

impl Default for BasinFixture {
    fn default() -> Self {
        Self::build()
    }
}

impl BasinFixture {
    pub fn build() -> Self {
        let mut cells = std::array::from_fn(|index| BasinCell {
            ground_y: DEM[index],
            spill_y: DEM[index],
            downstream: None,
            discharge: 1,
            flood_rank: 0,
        });
        let mut seen = [false; BASIN_CELLS];
        let mut queue = BinaryHeap::new();
        // Other boundary cells are watershed divides, not extra ocean outlets.
        queue.push(Reverse((DEM[BASIN_OUTLET], BASIN_OUTLET)));
        seen[BASIN_OUTLET] = true;
        let mut order = Vec::with_capacity(BASIN_CELLS);
        while let Some(Reverse((height, index))) = queue.pop() {
            cells[index].flood_rank = order.len();
            order.push(index);
            for neighbour in neighbours(index) {
                if seen[neighbour] {
                    continue;
                }
                seen[neighbour] = true;
                let spill = DEM[neighbour].max(height);
                cells[neighbour].spill_y = spill;
                cells[neighbour].downstream = Some(index);
                queue.push(Reverse((spill, neighbour)));
            }
        }
        // Parent popped before child: reverse flood order is topological.
        for &index in order.iter().rev() {
            if let Some(parent) = cells[index].downstream {
                cells[parent].discharge += cells[index].discharge;
            }
        }
        Self { cells }
    }

    pub fn cells(&self) -> &[BasinCell; BASIN_CELLS] {
        &self.cells
    }

    /// Fixed-width big-endian canonical root payload, not a derived cache.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(520);
        bytes.extend_from_slice(b"SBASIN01");
        for value in [
            BASIN_AXIS as u32,
            BASIN_CELL_METERS as u32,
            BASIN_OUTLET as u32,
        ] {
            bytes.extend_from_slice(&value.to_be_bytes());
        }
        for cell in &self.cells {
            for value in [
                cell.ground_y as u32,
                cell.spill_y as u32,
                cell.downstream.map_or(u32::MAX, |index| index as u32),
                cell.discharge,
                cell.flood_rank as u32,
            ] {
                bytes.extend_from_slice(&value.to_be_bytes());
            }
        }
        bytes
    }

    /// Decode bounded bytes and validate the complete DAG and water budget.
    pub fn from_canonical_bytes(bytes: &[u8]) -> Result<Self, &'static str> {
        if bytes.len() != 20 + BASIN_CELLS * 20 || &bytes[..8] != b"SBASIN01" {
            return Err("basin magic/version/length mismatch");
        }
        let word = |offset| u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap());
        if word(8) != BASIN_AXIS as u32
            || word(12) != BASIN_CELL_METERS as u32
            || word(16) != BASIN_OUTLET as u32
        {
            return Err("basin geometry mismatch");
        }
        let cells = std::array::from_fn(|index| {
            let offset = 20 + index * 20;
            let parent = word(offset + 8);
            BasinCell {
                ground_y: word(offset) as i32,
                spill_y: word(offset + 4) as i32,
                downstream: (parent != u32::MAX).then_some(parent as usize),
                discharge: word(offset + 12),
                flood_rank: word(offset + 16) as usize,
            }
        });
        let mut ranks = [false; BASIN_CELLS];
        for (index, cell) in cells.iter().enumerate() {
            if !(MIN_Y..mc_world::MAX_Y).contains(&cell.ground_y)
                || !(cell.ground_y..mc_world::MAX_Y).contains(&cell.spill_y)
                || cell.flood_rank >= BASIN_CELLS
                || ranks[cell.flood_rank]
            {
                return Err("invalid basin elevation/rank");
            }
            ranks[cell.flood_rank] = true;
            match cell.downstream {
                Some(parent) => {
                    if parent >= BASIN_CELLS
                        || !neighbours(index).any(|n| n == parent)
                        || cells[parent].flood_rank >= cell.flood_rank
                        || cells[parent].spill_y > cell.spill_y
                    {
                        return Err("invalid basin downstream reference");
                    }
                }
                None if index == BASIN_OUTLET => {}
                None => return Err("unexpected basin outlet"),
            }
            let inflow = cells
                .iter()
                .filter(|child| child.downstream == Some(index))
                .try_fold(1_u32, |total, child| total.checked_add(child.discharge))
                .ok_or("basin budget overflow")?;
            if cell.discharge != inflow {
                return Err("basin budget mismatch");
            }
        }
        if cells[BASIN_OUTLET].downstream.is_some()
            || cells[BASIN_OUTLET].discharge != BASIN_CELLS as u32
        {
            return Err("basin outlet mismatch");
        }
        Ok(Self { cells })
    }

    /// No IO or runtime mutation. Outside this finite fixture is absent,
    /// never an alternate noise base. Each chunk is exactly one plan cell.
    pub fn materialize_chunk(&self, position: ChunkPos, registry: &BlockRegistry) -> Option<Chunk> {
        if !(0..BASIN_AXIS as i32).contains(&position.x)
            || !(0..BASIN_AXIS as i32).contains(&position.z)
        {
            return None;
        }
        let state = |name: &str| {
            registry
                .block(&Identifier::parse(name).ok()?)
                .map(|block| block.default)
        };
        let air = state("minecraft:air")?;
        let stone = state("minecraft:stone")?;
        let water = state("minecraft:water")?;
        let index = position.z as usize * BASIN_AXIS + position.x as usize;
        let cell = self.cells[index];
        let mut chunk = Chunk::empty(position, air, Identifier::parse("minecraft:plains").ok()?);
        for z in 0..16_u8 {
            for x in 0..16_u8 {
                let lake = cell.spill_y > cell.ground_y;
                let reach = cell.discharge >= 3 && self.on_reach(index, x, z);
                let floor = if reach && !lake {
                    cell.ground_y - 2
                } else {
                    cell.ground_y
                };
                for y in MIN_Y..=floor {
                    chunk.set_block(x, y, z, stone)?;
                }
                if lake || reach {
                    for y in floor + 1..=cell.spill_y {
                        chunk.set_block(x, y, z, water)?;
                    }
                }
            }
        }
        chunk.status = "minecraft:full".into();
        // Pure base materialization is not an event in gameplay history.
        chunk.dirty = false;
        Some(chunk)
    }

    fn on_reach(&self, index: usize, x: u8, z: u8) -> bool {
        let centre = (7..=8).contains(&x) && (7..=8).contains(&z);
        centre
            || self.cells.iter().enumerate().any(|(other, cell)| {
                // Water-bearing parent/child segments share the exact edge port.
                if cell.discharge < 3 {
                    return false;
                }
                let Some(parent) = cell.downstream else {
                    return false;
                };
                if other == index {
                    toward(index, parent, x, z)
                } else if parent == index {
                    toward(index, other, x, z)
                } else {
                    false
                }
            })
    }
}

fn toward(from: usize, to: usize, x: u8, z: u8) -> bool {
    if to + BASIN_AXIS == from {
        (7..=8).contains(&x) && z <= 8
    } else if to == from + BASIN_AXIS {
        (7..=8).contains(&x) && z >= 7
    } else if to + 1 == from {
        (7..=8).contains(&z) && x <= 8
    } else {
        (7..=8).contains(&z) && x >= 7
    }
}

fn neighbours(index: usize) -> impl Iterator<Item = usize> {
    let x = index % BASIN_AXIS;
    let z = index / BASIN_AXIS;
    [
        (x > 0).then(|| index - 1),
        (x + 1 < BASIN_AXIS).then_some(index + 1),
        (z > 0).then(|| index - BASIN_AXIS),
        (z + 1 < BASIN_AXIS).then_some(index + BASIN_AXIS),
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spill_lake_and_complete_upstream_budget_constrain_every_cell() {
        let plan = BasinFixture::build();
        assert_eq!(plan.cells[12].ground_y, 82);
        assert_eq!(plan.cells[12].spill_y, 92);
        assert_eq!(plan.cells[12].downstream, Some(17));
        assert_eq!(plan.cells[BASIN_OUTLET].discharge, BASIN_CELLS as u32);
        assert_eq!(plan.cells[BASIN_OUTLET].downstream, None);
        for (index, cell) in plan.cells.iter().enumerate() {
            let inflow: u32 = plan
                .cells
                .iter()
                .filter(|child| child.downstream == Some(index))
                .map(|child| child.discharge)
                .sum();
            assert_eq!(cell.discharge, inflow + 1);
            if let Some(parent) = cell.downstream {
                assert!(cell.spill_y >= plan.cells[parent].spill_y);
                assert!(cell.flood_rank > plan.cells[parent].flood_rank);
            }
            let mut cursor = index;
            for _ in 0..BASIN_CELLS {
                match plan.cells[cursor].downstream {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
            assert_eq!(cursor, BASIN_OUTLET);
        }
    }

    #[test]
    fn lake_blocks_and_reach_ports_follow_the_plan_not_sea_level() {
        let registry =
            BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap();
        let water = registry
            .block(&Identifier::parse("minecraft:water").unwrap())
            .unwrap()
            .default;
        let plan = BasinFixture::build();
        let lake = plan
            .materialize_chunk(ChunkPos { x: 2, z: 2 }, &registry)
            .unwrap();
        assert_eq!(lake.get_block(1, 92, 1), Some(water));
        assert_ne!(lake.get_block(1, 93, 1), Some(water));
        assert!(!lake.dirty);
        for (index, cell) in plan.cells.iter().enumerate() {
            if cell.discharge < 3 {
                continue;
            }
            let Some(parent) = cell.downstream else {
                continue;
            };
            let from = plan
                .materialize_chunk(
                    ChunkPos {
                        x: (index % 5) as i32,
                        z: (index / 5) as i32,
                    },
                    &registry,
                )
                .unwrap();
            let to = plan
                .materialize_chunk(
                    ChunkPos {
                        x: (parent % 5) as i32,
                        z: (parent / 5) as i32,
                    },
                    &registry,
                )
                .unwrap();
            let (fx, fz, tx, tz) = if parent + 5 == index {
                (7, 0, 7, 15)
            } else if parent == index + 5 {
                (7, 15, 7, 0)
            } else if parent + 1 == index {
                (0, 7, 15, 7)
            } else {
                (15, 7, 0, 7)
            };
            assert_eq!(from.get_block(fx, cell.spill_y, fz), Some(water));
            assert_eq!(
                to.get_block(tx, plan.cells[parent].spill_y, tz),
                Some(water)
            );
        }
        assert!(
            plan.materialize_chunk(ChunkPos { x: -1, z: 0 }, &registry)
                .is_none()
        );
    }
}
