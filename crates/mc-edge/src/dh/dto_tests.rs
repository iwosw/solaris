//! Regression tests for the DH format-1 DTO encoder (`super`).
//!
//! Small asymmetric fixture on section (5,-3): grass base, a water pool
//! exposing the seabed, an overhanging canopy split by an air gap, and a
//! sky-light gradient along one row. Tests prove Z-fastest column order,
//! descending runs, property-sorted mapping serials, `writeUTF` semantics,
//! light nibble separation, Zstd envelope decoding, and fail-closed inputs.
//! Pure [`super::DhColumns`] checks keep services out of the loop.

use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos, ChunkSection, LightSection};

use super::{
    COMPRESSION_ZSTD_BLOCK, DATA_FORMAT_V1, DhBlockProjector, MAX_DTO_BYTES, SECTION_DETAIL,
    VerbatimProjector, canonical_serial, columns_for_section, decode_section, encode,
    encode_projected, write_java_utf,
};

const AIR: BlockStateId = BlockStateId(0);
const GRASS: BlockStateId = BlockStateId(1);
const WATER: BlockStateId = BlockStateId(3);
const LEAVES: BlockStateId = BlockStateId(4);
const STONE: BlockStateId = BlockStateId(5);
const TORCH: BlockStateId = BlockStateId(6);

type FixtureState<'a> = (u32, bool, Vec<(&'a str, &'a str)>);

fn fixture_registry() -> BlockRegistry {
    let block =
        |name: &str, props: &[(&str, &[&str])], states: Vec<FixtureState<'_>>| BlockReport {
            id: Identifier::parse(name).unwrap(),
            properties: props
                .iter()
                .map(|(k, v)| (k.to_string(), v.iter().map(|s| s.to_string()).collect()))
                .collect::<BTreeMap<_, _>>(),
            states: states
                .into_iter()
                .map(|(id, default, props)| BlockStateReport {
                    id,
                    default,
                    properties: props
                        .into_iter()
                        .map(|(k, v)| (k.to_string(), v.to_string()))
                        .collect(),
                })
                .collect(),
        };
    let report = vec![
        block("minecraft:air", &[], vec![(0, true, vec![])]),
        block(
            "minecraft:grass_block",
            &[("snowy", &["true", "false"][..])],
            vec![
                (1, true, vec![("snowy", "false")]),
                (2, false, vec![("snowy", "true")]),
            ],
        ),
        block(
            "minecraft:water",
            &[("level", &["0"][..])],
            vec![(3, true, vec![("level", "0")])],
        ),
        block("minecraft:oak_leaves", &[], vec![(4, true, vec![])]),
        block("minecraft:stone", &[], vec![(5, true, vec![])]),
        block(
            "minecraft:torch",
            &[("facing", &["north"][..]), ("lit", &["true"][..])],
            vec![(6, true, vec![("facing", "north"), ("lit", "true")])],
        ),
    ];
    BlockRegistry::from_report(&report).unwrap()
}

fn plains() -> Identifier {
    Identifier::parse("minecraft:plains").unwrap()
}

fn ocean() -> Identifier {
    Identifier::parse("minecraft:ocean").unwrap()
}

fn section_pos(section_x: i32, section_z: i32) -> i64 {
    i64::from(SECTION_DETAIL)
        | (i64::from(section_x as u32 & 0x0FFF_FFFF) << 8)
        | (i64::from(section_z as u32 & 0x0FFF_FFFF) << 36)
}

/// 4x4 chunks at section (5,-3): stone base to y=-1, grass at y=0, water
/// pool at rel (4..7,4..7) y=1..2, canopy at rel (10..13,2..5) y=8..9 with
/// an air gap below, torch landmark at rel (20,20) y=1, sky gradient along
/// rel_x==7 / rel_z 28..35 at y=0.
fn section_chunks() -> Vec<Arc<Chunk>> {
    let mut slabs = Vec::with_capacity(16);
    for dz in 0..4 {
        for dx in 0..4 {
            let mut chunk = Chunk::empty(
                ChunkPos {
                    x: 5 * 4 + dx,
                    z: -3 * 4 + dz,
                },
                AIR,
                plains(),
            );
            for s in 0..4 {
                chunk.sections[s] = ChunkSection::filled(STONE, AIR);
            }
            for lx in 0..16u8 {
                for lz in 0..16u8 {
                    chunk.set_block(lx, 0, lz, GRASS);
                }
            }
            slabs.push(chunk);
        }
    }
    let at = |rel_x: i32, rel_z: i32| -> (usize, u8, u8) {
        let idx = (rel_z / 16) * 4 + (rel_x / 16);
        ((idx) as usize, (rel_x % 16) as u8, (rel_z % 16) as u8)
    };
    let set = |slabs: &mut Vec<Chunk>, rel_x: i32, rel_z: i32, y: i32, id: BlockStateId| {
        let (idx, lx, lz) = at(rel_x, rel_z);
        slabs[idx].set_block(lx, y, lz, id);
    };
    for x in 4..7 {
        for z in 4..7 {
            set(&mut slabs, x, z, 1, WATER);
            set(&mut slabs, x, z, 2, WATER);
        }
    }
    for x in 10..13 {
        for z in 2..5 {
            set(&mut slabs, x, z, 8, LEAVES);
            set(&mut slabs, x, z, 9, LEAVES);
        }
    }
    set(&mut slabs, 20, 20, 1, TORCH);
    // Sky-light gradient 0..7 along rel_x==7, rel_z 28..35 at y=0.
    for (i, z) in (28..36).enumerate() {
        let (idx, lx, lz) = at(7, z);
        let chunk = &mut slabs[idx];
        let section = 4; // y=0 -> (0+64)/16
        let cell = (usize::from(lz)) * 16 + usize::from(lx); // sub_y=0
        let mut layer = chunk.section_lights[section]
            .sky
            .clone()
            .unwrap_or_else(|| LightSection::uniform(0));
        layer.set(cell, i as u8);
        chunk.section_lights[section].sky = Some(layer);
    }
    slabs.into_iter().map(Arc::new).collect()
}

#[test]
fn section_pos_round_trips_negative_coordinates() {
    let (x, z) = decode_section(section_pos(5, -3)).unwrap();
    assert_eq!((x, z), (5, -3));
    let (x, z) = decode_section(section_pos(-1, -1)).unwrap();
    assert_eq!((x, z), (-1, -1));
}

#[test]
fn rejects_non_leaf_detail() {
    assert!(decode_section(5).is_err());
    assert!(decode_section(7).is_err());
}
#[test]
fn column_order_is_z_fastest() {
    let registry = fixture_registry();
    let chunks = section_chunks();
    let cols = columns_for_section(&chunks, &registry, &VerbatimProjector, 5, -3).unwrap();
    assert_eq!(cols.columns.len(), 4096);
    assert_eq!(
        (chunks[0].geometry().min_y(), chunks[0].geometry().max_y()),
        (-64, 320)
    );
    let torch_map = cols
        .mapping
        .iter()
        .position(|m| m.contains("minecraft:torch"))
        .unwrap() as u64;
    let here: Vec<u64> = cols.columns[20 * 64 + 20]
        .iter()
        .filter(|r| *r & 0xFFFF_FFFF == torch_map)
        .copied()
        .collect();
    assert!(!here.is_empty(), "torch run missing at (20,20)");
    for (dx, dz) in [(19, 20), (21, 20), (20, 19), (20, 21)] {
        assert!(
            cols.columns[(dx * 64 + dz) as usize]
                .iter()
                .all(|r| r & 0xFFFF_FFFF != torch_map),
            "torch leaked into ({dx},{dz})"
        );
    }
}

#[test]
fn runs_descend_and_preserve_water_canopy_gaps() {
    let registry = fixture_registry();
    let chunks = section_chunks();
    let cols = columns_for_section(&chunks, &registry, &VerbatimProjector, 5, -3).unwrap();
    for (col, name) in [(10 * 64 + 2, "canopy"), (4 * 64 + 4, "water")] {
        let runs = &cols.columns[col as usize];
        assert!(runs.len() >= 3, "{name} column should split: {runs:?}");
        let mut top = chunks[0].geometry().max_y();
        for run in runs {
            let height = ((run >> 32) & 0xFFF) as i32;
            let bottom = ((run >> 44) & 0xFFF) as i32 + chunks[0].geometry().min_y();
            assert_eq!(bottom + height, top, "{name} runs must tile");
            top = bottom;
        }
        assert_eq!(top, chunks[0].geometry().min_y());
    }
    let water_map = cols
        .mapping
        .iter()
        .position(|m| m.contains("minecraft:water"))
        .unwrap() as u64;
    assert_eq!(
        cols.columns[(4 * 64 + 4) as usize][1] & 0xFFFF_FFFF,
        water_map
    );
    let leaves_map = cols
        .mapping
        .iter()
        .position(|m| m.contains("minecraft:oak_leaves"))
        .unwrap() as u64;
    assert!(
        cols.columns[(10 * 64 + 2) as usize]
            .iter()
            .any(|r| r & 0xFFFF_FFFF == leaves_map)
    );
    let lit: Vec<(u8, u8)> = cols.columns[(7 * 64 + 30) as usize]
        .iter()
        .map(|r| (((r >> 56) & 0xF) as u8, ((r >> 60) & 0xF) as u8))
        .collect();
    assert!(
        lit.windows(2).any(|w| w[0].0 != w[1].0),
        "sky gradient merged into one run"
    );
}

#[test]
fn mapping_sorts_properties_and_prefers_projector() {
    let registry = fixture_registry();
    let torch = registry.by_id(TORCH).unwrap();
    assert_eq!(
        canonical_serial(torch, &plains()),
        "minecraft:plains_DH-BSW_minecraft:torch_STATE_{facing:north}{lit:true}"
    );
    let stone = registry.by_id(STONE).unwrap();
    assert_eq!(
        canonical_serial(stone, &ocean()),
        "minecraft:ocean_DH-BSW_minecraft:stone_STATE_"
    );
    struct Rename;
    impl DhBlockProjector for Rename {
        fn project(&self, state: &mc_world::BlockState, biome: &Identifier) -> Option<String> {
            if state.block.id.as_str() == "minecraft:torch" {
                return Some(format!(
                    "{}_DH-BSW_minecraft:lantern_STATE_",
                    biome.as_str()
                ));
            }
            None
        }
    }
    let chunks = section_chunks();
    let cols = columns_for_section(&chunks, &registry, &Rename, 5, -3).unwrap();
    assert!(cols.mapping.iter().any(|m| m.contains("lantern")));
    assert!(!cols.mapping.iter().any(|m| m.contains("torch")));
    let _ = encode_projected(section_pos(5, -3), &chunks, &registry, &Rename, 7).unwrap();
}

#[test]
fn java_utf_matches_modified_encoding() {
    let mut buf = Vec::new();
    write_java_utf(&mut buf, "a\0水").unwrap();
    assert_eq!(buf, [0, 6, b'a', 0xC0, 0x80, 0xE6, 0xB0, 0xB4]);
}

#[test]
fn envelope_decodes_with_zstd_and_expected_flags() {
    let registry = fixture_registry();
    let chunks = section_chunks();
    let pos = section_pos(5, -3);
    let bytes = encode(pos, &chunks, &registry, 1_234).unwrap();
    assert!(bytes.len() < MAX_DTO_BYTES);

    let mut cur: &[u8] = &bytes;
    fn take<'a>(cur: &mut &'a [u8], n: usize) -> &'a [u8] {
        let (h, t) = cur.split_at(n);
        *cur = t;
        h
    }
    assert_eq!(
        i64::from_be_bytes(take(&mut cur, 8).try_into().unwrap()),
        pos
    );
    assert_eq!(i32::from_be_bytes(take(&mut cur, 4).try_into().unwrap()), 0);
    let mut blobs = Vec::new();
    for _ in 0..8 {
        let len = i32::from_be_bytes(take(&mut cur, 4).try_into().unwrap()) as usize;
        let (b, t) = cur.split_at(len);
        blobs.push(b.to_vec());
        cur = t;
    }
    for edge in [1usize, 2, 3, 4] {
        assert_eq!(blobs[edge].len(), 0, "edge blob {edge} must be empty");
    }
    let main = zstd::bulk::decompress(&blobs[0], 4 * 1024 * 1024).unwrap();
    let mut m: &[u8] = &main;
    for _ in 0..4096 {
        let n = i16::from_be_bytes(m[..2].try_into().unwrap()) as usize;
        m = &m[2..];
        for _ in 0..n {
            let run = u64::from_be_bytes(m[..8].try_into().unwrap());
            assert!((run >> 32) & 0xFFF >= 1);
            m = &m[8..];
        }
    }
    assert!(m.is_empty());
    let steps = zstd::bulk::decompress(&blobs[5], 8192).unwrap();
    assert_eq!(steps, vec![9; 4096]);
    let world = zstd::bulk::decompress(&blobs[6], 8192).unwrap();
    assert_eq!(world, vec![0; 4096]);
    let map_raw = zstd::bulk::decompress(&blobs[7], 1024 * 1024).unwrap();
    let count = i32::from_be_bytes(map_raw[..4].try_into().unwrap()) as usize;
    assert!(count > 0);
    assert_eq!(cur[0], DATA_FORMAT_V1);
    assert_eq!(cur[1], COMPRESSION_ZSTD_BLOCK);
    assert_eq!(&cur[2..5], &[1, 0, 0]);
    assert_eq!(i64::from_be_bytes(cur[5..13].try_into().unwrap()), 1_234);
    assert_eq!(i64::from_be_bytes(cur[13..21].try_into().unwrap()), 1_234);
    assert_eq!(cur.len(), 21);
}

#[test]
fn biome_transitions_split_runs() {
    let registry = fixture_registry();
    let mut chunk = Chunk::empty(ChunkPos { x: 20, z: -12 }, AIR, plains());
    for s in 0..4 {
        chunk.sections[s] = ChunkSection::filled(STONE, AIR);
    }
    chunk.set_block(0, 0, 0, GRASS);
    let mid = chunk.geometry().section_count() / 2;
    let mut bits = mc_world::PackedBitArray::zeroed(1, 64);
    for cell in 16..64 {
        bits.set(cell, 1);
    }
    chunk.biomes[mid] = mc_world::BiomeSection::from_indirect(vec![plains(), ocean()], bits);
    let mut padded = vec![Arc::new(chunk)];
    for dz in 0..4 {
        for dx in 0..4 {
            if dx == 0 && dz == 0 {
                continue;
            }
            padded.push(Arc::new(Chunk::empty(
                ChunkPos {
                    x: 5 * 4 + dx,
                    z: -3 * 4 + dz,
                },
                AIR,
                plains(),
            )));
        }
    }
    let cols = columns_for_section(&padded, &registry, &VerbatimProjector, 5, -3).unwrap();
    assert!(
        cols.mapping
            .iter()
            .any(|m| m.starts_with("minecraft:ocean"))
            && cols
                .mapping
                .iter()
                .any(|m| m.starts_with("minecraft:plains")),
        "biome change must split runs: {:?}",
        cols.mapping
    );
}

#[test]
fn missing_baked_light_reads_zero() {
    let registry = fixture_registry();
    let mut chunk = Chunk::empty(ChunkPos { x: 20, z: -12 }, AIR, plains());
    chunk.sections[0] = ChunkSection::filled(STONE, AIR);
    chunk.set_block(0, -64, 0, STONE);
    let chunks = vec![Arc::new(chunk)];
    let mut padded = chunks;
    for dz in 0..4 {
        for dx in 0..4 {
            if dx == 0 && dz == 0 {
                continue;
            }
            padded.push(Arc::new(Chunk::empty(
                ChunkPos {
                    x: 5 * 4 + dx,
                    z: -3 * 4 + dz,
                },
                AIR,
                plains(),
            )));
        }
    }
    let cols = columns_for_section(&padded, &registry, &VerbatimProjector, 5, -3).unwrap();
    let stone_map = cols
        .mapping
        .iter()
        .position(|m| m.contains("minecraft:stone"))
        .unwrap() as u64;
    let run = cols.columns[0]
        .iter()
        .find(|r| *r & 0xFFFF_FFFF == stone_map)
        .copied()
        .unwrap();
    assert_eq!((run >> 56) & 0xF, 0, "missing sky light reads 0");
    assert_eq!((run >> 60) & 0xF, 0, "missing block light reads 0");
}

#[test]
fn halo_chunks_are_ignored() {
    let registry = fixture_registry();
    let mut chunks = section_chunks();
    chunks.push(Arc::new(Chunk::empty(
        ChunkPos { x: 999, z: -999 },
        AIR,
        plains(),
    )));
    let cols = columns_for_section(&chunks, &registry, &VerbatimProjector, 5, -3).unwrap();
    assert_eq!(cols.columns.len(), 4096);
}

#[test]
fn missing_chunk_or_geometry_or_state_fails_closed() {
    let registry = fixture_registry();
    let chunks = section_chunks();
    assert!(columns_for_section(&chunks[1..], &registry, &VerbatimProjector, 5, -3).is_err());
    let mut bad_chunk = (*chunks[0]).clone();
    bad_chunk.set_block(0, 1, 0, BlockStateId(999));
    let mut bad = vec![Arc::new(bad_chunk)];
    bad.extend(chunks[1..].iter().cloned());
    assert!(columns_for_section(&bad, &registry, &VerbatimProjector, 5, -3).is_err());
    let narrow = Chunk::empty_with_geometry(
        ChunkPos { x: 20, z: -12 },
        AIR,
        plains(),
        mc_world::ChunkGeometry::new(0, 256).unwrap(),
    );
    let mut mixed = vec![Arc::new(narrow)];
    mixed.extend(chunks[1..].iter().cloned());
    assert!(columns_for_section(&mixed, &registry, &VerbatimProjector, 5, -3).is_err());
}
