use super::*;
use mc_data::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;

#[test]
fn compact_block_palettes_round_trip_through_vanilla_anvil_words() {
    let report = (0..257)
        .map(|state| BlockReport {
            id: Identifier::parse(format!("minecraft:state_{state}")).unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: state,
                default: true,
                properties: BTreeMap::new(),
            }],
        })
        .collect::<Vec<_>>();
    let registry = BlockRegistry::from_report(&report).unwrap();
    let air = BlockStateId(0);
    for palette_len in [1usize, 2, 3, 5, 9, 17, 257] {
        let mut section = ChunkSection::filled(air, air);
        for cell in 0..SECTION_VOLUME {
            section.set(
                (cell % 16) as u8,
                (cell / 256) as u8,
                ((cell / 16) % 16) as u8,
                BlockStateId((cell % palette_len) as u32),
            );
        }
        let encoded = encode_block_section(&section, &registry).unwrap();
        let fields = expect_compound(&encoded, "block_states").unwrap();
        let palette = get_list(fields, "palette").unwrap();
        assert_eq!(palette.elements.len(), palette_len);
        if palette_len == 1 {
            assert!(get_optional_long_array(fields, "data").unwrap().is_none());
        } else {
            let bits = ((palette_len - 1).ilog2() as usize + 1).max(4);
            let entries_per_word = 64 / bits;
            // Independent vanilla packing oracle: no entry crosses a long.
            let mut expected = vec![0i64; SECTION_VOLUME.div_ceil(entries_per_word)];
            for cell in 0..SECTION_VOLUME {
                expected[cell / entries_per_word] |=
                    ((cell % palette_len) as i64) << ((cell % entries_per_word) * bits);
            }
            assert_eq!(
                get_optional_long_array(fields, "data").unwrap().unwrap(),
                expected
            );
        }
        let decoded = decode_block_section(fields, &registry, air).unwrap();
        assert_eq!(decoded.non_air_count(), section.non_air_count());
        let expected_ram_bits = if palette_len == 1 {
            0
        } else {
            (palette_len - 1).ilog2() as u8 + 1
        };
        assert_eq!(
            decoded.indices().map_or(0, PackedBitArray::bits_per_entry),
            expected_ram_bits
        );
        for cell in 0..SECTION_VOLUME {
            assert_eq!(
                decoded.get(
                    (cell % 16) as u8,
                    (cell / 256) as u8,
                    ((cell / 16) % 16) as u8
                ),
                BlockStateId((cell % palette_len) as u32)
            );
        }
        assert_eq!(encode_block_section(&decoded, &registry).unwrap(), encoded);
    }
}

#[test]
fn anvil_palette_missing_properties_inherit_block_defaults() {
    let registry = BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:wither_skeleton_wall_skull").unwrap(),
        properties: BTreeMap::from([
            ("facing".into(), vec!["north".into(), "west".into()]),
            ("powered".into(), vec!["true".into(), "false".into()]),
        ]),
        states: (0..4)
            .map(|id| BlockStateReport {
                id,
                default: id == 1,
                properties: BTreeMap::from([
                    (
                        "facing".into(),
                        if id < 2 { "north" } else { "west" }.into(),
                    ),
                    (
                        "powered".into(),
                        if id % 2 == 0 { "true" } else { "false" }.into(),
                    ),
                ]),
            })
            .collect(),
    }])
    .unwrap();
    let decode = |properties| {
        decode_block_section(
            &[(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: tag_type::COMPOUND,
                    elements: vec![Tag::Compound(vec![
                        (
                            "Name".into(),
                            Tag::String("minecraft:wither_skeleton_wall_skull".into()),
                        ),
                        ("Properties".into(), Tag::Compound(properties)),
                    ])],
                }),
            )],
            &registry,
            BlockStateId(0),
        )
    };
    let section = decode(vec![("facing".into(), Tag::String("west".into()))]).unwrap();
    assert_eq!(section.get(0, 0, 0), BlockStateId(3));
    assert_eq!(decode(Vec::new()).unwrap().get(0, 0, 0), BlockStateId(1));
    assert!(matches!(
        decode(vec![("facing".into(), Tag::String("invalid".into()))]),
        Err(ChunkNbtError::UnknownBlockState { .. })
    ));
}

#[test]
fn malformed_palettes_fail_closed_instead_of_panicking_or_replacing_cells() {
    let registry = BlockRegistry::from_report(&[BlockReport {
        id: Identifier::parse("minecraft:air").unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id: 0,
            default: true,
            properties: BTreeMap::new(),
        }],
    }])
    .unwrap();
    for biome in [false, true] {
        for (length, data) in [
            (0, None),
            (2, None),
            (3, Some(vec![-1; if biome { 2 } else { 256 }])),
        ] {
            let entry = if biome {
                Tag::String("minecraft:plains".into())
            } else {
                Tag::Compound(vec![("Name".into(), Tag::String("minecraft:air".into()))])
            };
            let mut fields = vec![(
                "palette".into(),
                Tag::List(ListTag {
                    element_type: if biome {
                        tag_type::STRING
                    } else {
                        tag_type::COMPOUND
                    },
                    elements: vec![entry; length],
                }),
            )];
            if let Some(data) = data {
                fields.push(("data".into(), Tag::LongArray(data)));
            }
            let error = if biome {
                decode_biome_section(&fields).unwrap_err()
            } else {
                decode_block_section(&fields, &registry, BlockStateId(0)).unwrap_err()
            };
            assert!(
                matches!(error, ChunkNbtError::InvalidPalette { .. }),
                "{error}"
            );
        }
    }
}

#[test]
fn three_dimensional_biomes_roundtrip_every_cell() {
    let palette = [
        "minecraft:plains",
        "minecraft:lush_caves",
        "minecraft:desert",
    ]
    .map(|name| Tag::String(name.into()));
    let mut data = vec![0_i64; 2];
    for cell in 0..BIOME_VOLUME {
        data[cell / 32] |= ((cell % 3) as i64) << ((cell % 32) * 2);
    }
    let source = Tag::Compound(vec![
        (
            "palette".into(),
            Tag::List(ListTag {
                element_type: tag_type::STRING,
                elements: palette.to_vec(),
            }),
        ),
        ("data".into(), Tag::LongArray(data)),
    ]);
    let decoded = decode_biome_section(expect_compound(&source, "biomes").unwrap()).unwrap();
    assert_eq!(encode_biome_section(&decoded), source);
}
