//! Paletted-container rewriting: single, indirect and global palettes.

use mc_protocol::codec::{ReadMc, WriteMc};

use super::*;

fn ids() -> IdMap {
    // canonical 0 air, 1 stone, 40_000 cinnabar -> legacy 0, 1, 6 (granite)
    IdMap::by_name(
        [("air", 0), ("stone", 1), ("cinnabar", 40_000)],
        [("air", 0), ("stone", 1), ("granite", 6)],
        |name| (name == "cinnabar").then(|| "granite".to_owned()),
    )
}

fn pack(bits: u8, values: &[u64]) -> Vec<u8> {
    let per = 64 / usize::from(bits);
    let mut out = Vec::new();
    for chunk in values.chunks(per) {
        let mut long = 0u64;
        for (slot, value) in chunk.iter().enumerate() {
            long |= value << (slot * usize::from(bits));
        }
        out.write_u64(long);
    }
    out
}

fn unpack(bits: u8, mut data: &[u8], count: usize) -> Vec<u64> {
    let per = 64 / usize::from(bits);
    let mut values = Vec::new();
    while values.len() < count {
        let long = data.read_u64().unwrap();
        for slot in 0..per {
            if values.len() < count {
                values.push((long >> (slot * usize::from(bits))) & ((1 << bits) - 1));
            }
        }
    }
    values
}

#[test]
fn single_value_section_is_remapped() {
    let ids = ids();
    let mut input = Vec::new();
    input.write_u8(0);
    input.write_varint(40_000);
    let mut out = Vec::new();
    Container::blocks(&ids, 15)
        .rewrite(&mut &input[..], &mut out)
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_u8().unwrap(), 0);
    assert_eq!(r.read_varint().unwrap(), 6);
    assert!(r.is_empty());
}

#[test]
fn indirect_palette_keeps_indices() {
    let ids = ids();
    let indices: Vec<u64> = (0..4096).map(|i| (i % 3) as u64).collect();
    let mut input = Vec::new();
    input.write_u8(4);
    input.write_varint(3);
    for id in [0, 40_000, 1] {
        input.write_varint(id);
    }
    let data = pack(4, &indices);
    input.extend_from_slice(&data);
    let mut out = Vec::new();
    Container::blocks(&ids, 15)
        .rewrite(&mut &input[..], &mut out)
        .unwrap();
    let mut r = &out[..];
    assert_eq!(r.read_u8().unwrap(), 4);
    assert_eq!(r.read_varint().unwrap(), 3);
    assert_eq!(
        [
            r.read_varint().unwrap(),
            r.read_varint().unwrap(),
            r.read_varint().unwrap()
        ],
        [0, 6, 1]
    );
    assert_eq!(r, &data[..]);
}

#[test]
fn global_palette_is_repacked_at_legacy_width() {
    let ids = ids();
    let cells: Vec<u64> = (0..4096).map(|i| [0, 1, 40_000][i % 3]).collect();
    let mut input = Vec::new();
    input.write_u8(16);
    input.extend_from_slice(&pack(16, &cells));
    // A biome container follows in the same section.
    input.write_u8(0);
    input.write_varint(2);
    let mut section = vec![0, 1, 0, 0];
    section.extend_from_slice(&input);
    let biomes = IdMap::by_name([("plains", 2)], [("plains", 3)], |_| None);
    let mut out = Vec::new();
    rewrite_sections(
        &section,
        &Container::blocks(&ids, 15),
        &Container::biomes(&biomes, 7),
        &mut out,
    )
    .unwrap();
    assert_eq!(&out[..4], &[0, 1, 0, 0], "section counters are copied");
    let mut r = &out[4..];
    assert_eq!(r.read_u8().unwrap(), 15);
    let words = 4096_usize.div_ceil(64 / 15);
    let values = unpack(15, &r[..words * 8], 4096);
    assert!(
        values
            .iter()
            .enumerate()
            .all(|(i, v)| *v == [0, 1, 6][i % 3])
    );
    r = &r[words * 8..];
    assert_eq!(r.read_u8().unwrap(), 0);
    assert_eq!(r.read_varint().unwrap(), 3);
    assert!(r.is_empty());
}
