//! Vanilla 26.1.2 context-free collision boxes for every embedded block state.
//!
//! Entity-dependent rules such as leather boots walking on powder snow belong
//! in the entity collision layer on top of this static baseline.

use std::sync::OnceLock;

use crate::Identifier;

const RAW_COLLISION_SHAPES: &[u8] = include_bytes!("../data/block_collision_shapes_26_1_2.bin");
const EXPECTED_VERSION: &str = "26.1.2";
pub const COLLISION_UNITS_PER_BLOCK: i16 = 4096;
const HEADER_LEN: usize = 32;
const MISSING_SHAPE: u16 = u16::MAX;
static VANILLA_COLLISION_SHAPES: OnceLock<CollisionShapeTable> = OnceLock::new();
static VANILLA_COLLISION_CLASSES: OnceLock<Box<[CollisionClass]>> = OnceLock::new();

/// Coarse collision classification for a single block state, derived from the
/// exact [`CollisionShapeTable`] data without any block-name heuristics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CollisionClass {
    /// The state id is outside the table or has no recorded shape.
    Missing,
    /// The recorded shape contains no collision boxes.
    Empty,
    /// The recorded shape is exactly one full-block cube.
    FullCube,
    /// Any other recorded shape.
    Complex,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CollisionBox {
    coordinates: [i16; 6],
}

impl CollisionBox {
    #[must_use]
    pub const fn new(
        min_x: i16,
        min_y: i16,
        min_z: i16,
        max_x: i16,
        max_y: i16,
        max_z: i16,
    ) -> Self {
        assert!(min_x < max_x && min_y < max_y && min_z < max_z);
        Self {
            coordinates: [min_x, min_y, min_z, max_x, max_y, max_z],
        }
    }

    #[must_use]
    pub const fn coordinates(self) -> [i16; 6] {
        self.coordinates
    }

    #[must_use]
    pub fn as_blocks(self) -> [f64; 6] {
        self.coordinates
            .map(|value| f64::from(value) / f64::from(COLLISION_UNITS_PER_BLOCK))
    }
}

#[derive(Debug)]
pub struct CollisionShapeTable {
    bytes: &'static [u8],
    state_count: usize,
    shape_count: usize,
    box_count: usize,
    fingerprints_offset: usize,
    shape_offsets_offset: usize,
    boxes_offset: usize,
    max_box_y: i16,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CollisionShape<'a> {
    bytes: &'a [u8],
}

impl<'a> CollisionShape<'a> {
    #[must_use]
    pub const fn len(self) -> usize {
        self.bytes.len() / 12
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.bytes.is_empty()
    }

    pub fn iter(self) -> impl ExactSizeIterator<Item = CollisionBox> + 'a {
        self.bytes
            .as_chunks::<12>()
            .0
            .iter()
            .map(|bytes| CollisionBox {
                coordinates: [
                    read_i16(bytes, 0),
                    read_i16(bytes, 2),
                    read_i16(bytes, 4),
                    read_i16(bytes, 6),
                    read_i16(bytes, 8),
                    read_i16(bytes, 10),
                ],
            })
    }

    #[must_use]
    pub fn is_full_cube(self) -> bool {
        self.len() == 1
            && self.iter().next().is_some_and(|collision_box| {
                collision_box.coordinates()
                    == [
                        0,
                        0,
                        0,
                        COLLISION_UNITS_PER_BLOCK,
                        COLLISION_UNITS_PER_BLOCK,
                        COLLISION_UNITS_PER_BLOCK,
                    ]
            })
    }

    /// Covered half-block squares on a boundary face (axis X/Y/Z).
    /// Slab/stair boxes align with this grid; no allocation or raster scan.
    #[must_use]
    pub fn face_quadrants(self, axis: usize, positive: bool) -> u8 {
        let boundary = if positive {
            COLLISION_UNITS_PER_BLOCK
        } else {
            0
        };
        let a = (axis + 1) % 3;
        let b = (axis + 2) % 3;
        let half = COLLISION_UNITS_PER_BLOCK / 2;
        let mut mask = 0;
        for bounds in self.iter().map(CollisionBox::coordinates) {
            if bounds[axis + if positive { 3 } else { 0 }] != boundary {
                continue;
            }
            for u in 0..2 {
                for v in 0..2 {
                    if bounds[a] <= u * half
                        && bounds[a + 3] >= (u + 1) * half
                        && bounds[b] <= v * half
                        && bounds[b + 3] >= (v + 1) * half
                    {
                        mask |= 1 << (u * 2 + v);
                    }
                }
            }
        }
        mask
    }
}

impl CollisionShapeTable {
    #[must_use]
    pub fn version(&self) -> &str {
        EXPECTED_VERSION
    }

    #[must_use]
    pub fn get(&self, state_id: u32) -> Option<CollisionShape<'_>> {
        let state_id = usize::try_from(state_id).ok()?;
        if state_id >= self.state_count {
            return None;
        }
        let shape = read_u16(self.bytes, HEADER_LEN + state_id * 2);
        if shape == MISSING_SHAPE {
            return None;
        }
        self.shape(shape as usize)
    }

    #[must_use]
    pub fn get_for_state(
        &self,
        state_id: u32,
        block: &Identifier,
        properties: &[(String, String)],
    ) -> Option<CollisionShape<'_>> {
        let index = usize::try_from(state_id).ok()?;
        if index >= self.state_count
            || read_u64(self.bytes, self.fingerprints_offset + index * 8)
                != state_fingerprint(block, properties)
        {
            return None;
        }
        self.get(state_id)
    }

    /// Checks exact farmland semantics independently of a state's numeric ID.
    #[must_use]
    pub fn is_exact_farmland_state(
        &self,
        block: &Identifier,
        properties: &[(String, String)],
    ) -> bool {
        block.as_str() == "minecraft:farmland"
            && properties.len() == 1
            && properties.iter().any(|(name, value)| {
                name == "moisture" && value.parse::<u8>().is_ok_and(|value| value <= 7)
            })
    }

    #[must_use]
    pub fn covered_state_count(&self) -> usize {
        self.state_count
    }

    #[must_use]
    pub const fn max_box_y(&self) -> i16 {
        self.max_box_y
    }

    #[must_use]
    pub fn max_box_y_blocks(&self) -> f64 {
        f64::from(self.max_box_y) / f64::from(COLLISION_UNITS_PER_BLOCK)
    }

    pub(crate) fn from_binary(bytes: &'static [u8]) -> Self {
        assert!(
            bytes.len() >= HEADER_LEN,
            "collision table header is truncated"
        );
        assert_eq!(&bytes[..8], b"SOLCOLL1", "collision table magic mismatch");
        assert_eq!(read_i16(bytes, 8), COLLISION_UNITS_PER_BLOCK);
        assert_eq!(read_u32(bytes, 24), 1, "unsupported collision table format");
        assert_eq!(
            read_u32(bytes, 28),
            0,
            "collision table reserved field is set"
        );
        let state_count = read_u32(bytes, 12) as usize;
        let shape_count = read_u32(bytes, 16) as usize;
        let box_count = read_u32(bytes, 20) as usize;
        let fingerprints_offset = HEADER_LEN + state_count * 2;
        let shape_offsets_offset = fingerprints_offset + state_count * 8;
        let boxes_offset = shape_offsets_offset + (shape_count + 1) * 4;
        let expected_len = boxes_offset + box_count * 12;
        assert_eq!(bytes.len(), expected_len, "collision table length mismatch");
        assert_eq!(
            read_u32(bytes, shape_offsets_offset + shape_count * 4) as usize,
            box_count,
            "collision table terminal box offset mismatch"
        );
        for state in 0..state_count {
            let shape = read_u16(bytes, HEADER_LEN + state * 2);
            assert!(
                shape == MISSING_SHAPE || usize::from(shape) < shape_count,
                "collision table state shape index is out of range"
            );
        }
        let mut previous_offset = 0;
        for shape in 0..=shape_count {
            let offset = read_u32(bytes, shape_offsets_offset + shape * 4) as usize;
            assert!(
                offset >= previous_offset && offset <= box_count,
                "collision table shape offsets are invalid"
            );
            previous_offset = offset;
        }
        let mut actual_max_y = 0;
        for index in 0..box_count {
            let offset = boxes_offset + index * 12;
            let coordinates = [
                read_i16(bytes, offset),
                read_i16(bytes, offset + 2),
                read_i16(bytes, offset + 4),
                read_i16(bytes, offset + 6),
                read_i16(bytes, offset + 8),
                read_i16(bytes, offset + 10),
            ];
            assert!(
                coordinates[0] < coordinates[3]
                    && coordinates[1] < coordinates[4]
                    && coordinates[2] < coordinates[5],
                "collision table contains an invalid box"
            );
            actual_max_y = actual_max_y.max(coordinates[4]);
        }
        assert_eq!(
            read_i16(bytes, 10),
            actual_max_y,
            "collision table maximum Y mismatch"
        );
        Self {
            bytes,
            state_count,
            shape_count,
            box_count,
            fingerprints_offset,
            shape_offsets_offset,
            boxes_offset,
            max_box_y: read_i16(bytes, 10),
        }
    }

    fn shape(&self, shape: usize) -> Option<CollisionShape<'_>> {
        if shape >= self.shape_count {
            return None;
        }
        let start = read_u32(self.bytes, self.shape_offsets_offset + shape * 4) as usize;
        let end = read_u32(self.bytes, self.shape_offsets_offset + (shape + 1) * 4) as usize;
        if start > end || end > self.box_count {
            return None;
        }
        Some(CollisionShape {
            bytes: &self.bytes[self.boxes_offset + start * 12..self.boxes_offset + end * 12],
        })
    }
}

#[must_use]
pub fn vanilla_collision_shapes() -> &'static CollisionShapeTable {
    VANILLA_COLLISION_SHAPES.get_or_init(|| CollisionShapeTable::from_binary(RAW_COLLISION_SHAPES))
}

/// Returns the [`CollisionClass`] for a block state id in O(1) after one-time
/// lazy initialization of a dense per-state class table.
#[must_use]
pub fn vanilla_collision_class(state_id: u32) -> CollisionClass {
    let classes = VANILLA_COLLISION_CLASSES.get_or_init(|| {
        let table = vanilla_collision_shapes();
        (0..table.covered_state_count() as u32)
            .map(|state| classify_collision_shape(table.get(state)))
            .collect()
    });
    classes
        .get(state_id as usize)
        .copied()
        .unwrap_or(CollisionClass::Missing)
}

fn classify_collision_shape(shape: Option<CollisionShape<'_>>) -> CollisionClass {
    match shape {
        None => CollisionClass::Missing,
        Some(shape) if shape.is_empty() => CollisionClass::Empty,
        Some(shape) if shape.is_full_cube() => CollisionClass::FullCube,
        Some(_) => CollisionClass::Complex,
    }
}

fn state_fingerprint(block: &Identifier, properties: &[(String, String)]) -> u64 {
    let mut hash = fnv1a(0xcbf2_9ce4_8422_2325, block.as_str().as_bytes());
    for (name, value) in properties {
        hash = fnv1a(hash, b"\0");
        hash = fnv1a(hash, name.as_bytes());
        hash = fnv1a(hash, b"=");
        hash = fnv1a(hash, value.as_bytes());
    }
    hash
}

fn fnv1a(mut hash: u64, bytes: &[u8]) -> u64 {
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

fn read_u16(bytes: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("u16 slice"))
}

fn read_i16(bytes: &[u8], offset: usize) -> i16 {
    i16::from_be_bytes(bytes[offset..offset + 2].try_into().expect("i16 slice"))
}

fn read_u32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(bytes[offset..offset + 4].try_into().expect("u32 slice"))
}

fn read_u64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(bytes[offset..offset + 8].try_into().expect("u64 slice"))
}

#[cfg(test)]
#[path = "collision_shapes_tests.rs"]
mod tests;
