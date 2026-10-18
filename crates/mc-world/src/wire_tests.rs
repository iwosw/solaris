use super::*;
use crate::block::BlockStateId;
use crate::chunk::{Chunk, ChunkGeometry, ChunkPos, FurnaceSlot, Heightmap};
use mc_data::Registry;
use mc_data::blocks::{BlockReport, BlockStateReport};
use mc_data::items::{ItemRegistry, ItemReport};
use std::collections::BTreeMap;

const AIR: BlockStateId = BlockStateId(0);
const BEDROCK: BlockStateId = BlockStateId(74);

fn biome_registry() -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
        entries: vec![
            Identifier::parse("minecraft:badlands").unwrap(),
            Identifier::parse("minecraft:plains").unwrap(),
            Identifier::parse("minecraft:the_void").unwrap(),
        ],
    }
}

fn numbered_biome_registry(len: usize) -> Registry {
    Registry {
        id: Identifier::parse("minecraft:worldgen/biome").unwrap(),
        entries: (0..len)
            .map(|i| Identifier::parse(format!("minecraft:biome_{i}")).unwrap())
            .collect(),
    }
}

fn empty_chunk() -> Chunk {
    Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        AIR,
        Identifier::parse("minecraft:plains").unwrap(),
    )
}

fn air_chest_registry() -> BlockRegistry {
    BlockRegistry::from_report(&[
        BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:chest").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 1,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
    ])
    .unwrap()
}

fn fluid_block_registry() -> BlockRegistry {
    let single = |id: u32, name: &str| BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id,
            default: true,
            properties: BTreeMap::new(),
        }],
    };
    let waterlogged = BlockReport {
        id: Identifier::parse("minecraft:oak_fence").unwrap(),
        properties: BTreeMap::from([(
            "waterlogged".to_string(),
            vec!["false".to_string(), "true".to_string()],
        )]),
        states: [false, true]
            .into_iter()
            .enumerate()
            .map(|(offset, value)| BlockStateReport {
                id: 3 + offset as u32,
                default: !value,
                properties: BTreeMap::from([("waterlogged".to_string(), value.to_string())]),
            })
            .collect(),
    };
    BlockRegistry::from_report(&[
        single(0, "minecraft:air"),
        single(1, "minecraft:water"),
        single(2, "minecraft:kelp"),
        waterlogged,
        single(5, "minecraft:stone"),
        single(6, "minecraft:lava"),
        single(7, "minecraft:bubble_column"),
        single(8, "minecraft:kelp_plant"),
        single(9, "minecraft:seagrass"),
        single(10, "minecraft:tall_seagrass"),
    ])
    .unwrap()
}

fn item_registry() -> ItemRegistry {
    ItemRegistry::from_report(&[ItemReport {
        id: Identifier::parse("minecraft:stone").unwrap(),
        protocol_id: 9,
    }])
}

// ---- byte-level assertions ----

#[test]
fn empty_chunk_emits_single_value_air_sections() {
    let chunk = empty_chunk();
    let bytes = encode_chunk_data(&chunk, &biome_registry(), &air_chest_registry()).unwrap();

    // Per section: i16(0) i16(0) | u8(0) VarInt(0) | u8(0) VarInt(1)
    //            = 2 + 2 + 1 + 1 + 1 + 1 = 8 bytes.
    // 24 sections × 8 = 192 bytes total.
    assert_eq!(bytes.len(), 24 * 8);

    let expected: [u8; 8] = [
        0x00, 0x00, // non_air_count = 0
        0x00, 0x00, // fluid_count = 0
        0x00, // block bpe = 0
        0x00, // block VarInt(0) = AIR
        0x00, // biome bpe = 0
        0x01, // biome VarInt(1) = plains
    ];
    for sec in 0..24 {
        let start = sec * 8;
        assert_eq!(
            &bytes[start..start + 8],
            &expected,
            "section {sec} mismatches expected all-air/all-plains layout"
        );
    }
}

#[test]
fn chunk_palette_projects_one_server_owned_state_to_the_client_carrier() {
    let geometry = ChunkGeometry::new(0, 16).unwrap();
    let mut chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        AIR,
        Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    );
    chunk.sections[0] = ChunkSection::filled(BlockStateId(11), AIR);

    let bytes = encode_chunk_data_with_block_projection(
        &chunk,
        &biome_registry(),
        &air_chest_registry(),
        &|state| {
            if state == BlockStateId(11) {
                BlockStateId(42)
            } else {
                state
            }
        },
    )
    .unwrap();

    assert_eq!(bytes[4], 0, "single-value block palette");
    assert_eq!(bytes[5], 42, "projected carrier state VarInt");
}

#[test]
fn chunk_section_reports_exact_fluid_count_to_client() {
    let geometry = ChunkGeometry::new(0, 16).unwrap();
    let mut chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        AIR,
        Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    );
    chunk.set_block(0, 0, 0, BlockStateId(1)).unwrap(); // water
    chunk.set_block(1, 0, 0, BlockStateId(2)).unwrap(); // kelp
    chunk.set_block(2, 0, 0, BlockStateId(3)).unwrap(); // dry fence
    chunk.set_block(3, 0, 0, BlockStateId(4)).unwrap(); // waterlogged fence
    chunk.set_block(4, 0, 0, BlockStateId(5)).unwrap(); // stone
    chunk.set_block(5, 0, 0, BlockStateId(6)).unwrap(); // lava
    chunk.set_block(6, 0, 0, BlockStateId(7)).unwrap(); // bubble column
    chunk.set_block(7, 0, 0, BlockStateId(8)).unwrap(); // kelp plant
    chunk.set_block(8, 0, 0, BlockStateId(9)).unwrap(); // seagrass
    chunk.set_block(9, 0, 0, BlockStateId(10)).unwrap(); // tall seagrass

    let bytes = encode_chunk_data(&chunk, &biome_registry(), &fluid_block_registry()).unwrap();

    assert_eq!(i16::from_be_bytes([bytes[0], bytes[1]]), 10);
    assert_eq!(i16::from_be_bytes([bytes[2], bytes[3]]), 8);
}

#[test]
fn single_state_fluid_section_reports_full_count() {
    use crate::section::ChunkSection;

    let section = ChunkSection::filled(BlockStateId(1), AIR);
    assert_eq!(section_fluid_count(&section, &fluid_block_registry()), 4096);
}

#[test]
fn custom_geometry_encodes_chunk_data_and_baked_light_sections() {
    let geometry = ChunkGeometry::new(0, 256).expect("valid custom geometry");
    let mut chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        AIR,
        Identifier::parse("minecraft:plains").unwrap(),
        geometry,
    );

    let data = encode_chunk_data(&chunk, &biome_registry(), &air_chest_registry())
        .expect("encode chunk data");
    assert_eq!(data.len(), geometry.section_count() * 8);

    chunk.section_lights[15].block = Some(crate::chunk::LightSection::uniform(0x0F));
    let light = ChunkLight::from_section_lights(&chunk.section_lights)
        .expect("rebuild baked custom-geometry light");
    let wire = encode_chunk_light(&light);

    assert_eq!(wire.block_y_mask, vec![1 << 16]);
    assert_eq!(wire.block_updates.len(), 1);
    assert_eq!(wire.block_updates[0], vec![0x0F; LIGHT_LAYER_BYTES]);
    assert_eq!(wire.sky_y_mask, vec![1 << 17]);
    assert_eq!(wire.sky_updates, vec![vec![0xFF; LIGHT_LAYER_BYTES]]);
    assert_eq!(wire.empty_block_y_mask, vec![(1 << 18) - 1 - (1 << 16)]);
    assert_eq!(wire.empty_sky_y_mask, vec![(1 << 18) - 1 - (1 << 17)]);
}

#[test]
fn client_block_entities_emit_stripped_chest_update_tag() {
    let registry = air_chest_registry();
    let items = item_registry();
    let mut chunk = empty_chunk();
    let pos = BlockPos { x: 2, y: 64, z: 3 };
    chunk.set_block(2, 64, 3, BlockStateId(1)).unwrap();
    let mut chest = ChestBlockEntity::default();
    chest.slots[0] = FurnaceSlot {
        count: 2,
        item_id: 9,
        damage: None,
        enchantments: Vec::new(),
        custom_name: None,
        item_model: None,
        stew_effects: Vec::new(),
        components: None,
    };
    chunk.chests.insert(pos, chest);

    let records = client_block_entities(&chunk, &registry, &items);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].pos, pos);
    assert_eq!(records[0].type_name.as_str(), "minecraft:chest");
    let Tag::Compound(fields) = &records[0].nbt else {
        panic!("expected compound update tag");
    };
    assert!(
        fields
            .iter()
            .all(|(key, _)| !matches!(key.as_str(), "id" | "x" | "y" | "z"))
    );
    assert!(fields.iter().any(|(key, _)| key == "Items"));
}

#[test]
fn client_block_entities_strip_solaris_campfire_persistence_fields() {
    let registry = air_chest_registry();
    let items = item_registry();
    let mut chunk = empty_chunk();
    let pos = BlockPos { x: 2, y: 64, z: 3 };
    let tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:campfire".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        (
            "Items".into(),
            Tag::List(ListTag {
                element_type: tag_type::END,
                elements: Vec::new(),
            }),
        ),
        ("CookingTimes".into(), Tag::IntArray(vec![1, 0, 0, 0])),
        ("CookingTotalTimes".into(), Tag::IntArray(vec![4, 0, 0, 0])),
        (
            "solaris_cooking_remaining".into(),
            Tag::IntArray(vec![3, 0, 0, 0]),
        ),
        (
            "solaris_cooking_total".into(),
            Tag::IntArray(vec![4, 0, 0, 0]),
        ),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).expect("encode campfire block entity");
    chunk.block_entities.insert(pos, bytes);

    let records = client_block_entities(&chunk, &registry, &items);

    assert_eq!(records.len(), 1);
    assert_eq!(records[0].type_name.as_str(), "minecraft:campfire");
    let Tag::Compound(fields) = &records[0].nbt else {
        panic!("expected compound update tag");
    };
    assert!(fields.iter().any(|(key, _)| key == "Items"));
    assert!(fields.iter().all(|(key, _)| {
        !matches!(
            key.as_str(),
            "id" | "x"
                | "y"
                | "z"
                | "CookingTimes"
                | "CookingTotalTimes"
                | "solaris_cooking_remaining"
                | "solaris_cooking_total"
        )
    }));
}

fn air_bed_registry() -> BlockRegistry {
    let facing = ["north", "south", "west", "east"];
    let part = ["head", "foot"];
    let occupied = ["true", "false"];
    let mut states = Vec::new();
    let mut id = 1;
    for is_occupied in &occupied {
        for bed_part in &part {
            for direction in &facing {
                states.push(BlockStateReport {
                    id,
                    default: id == 1,
                    properties: BTreeMap::from([
                        ("facing".to_string(), (*direction).to_string()),
                        ("part".to_string(), (*bed_part).to_string()),
                        ("occupied".to_string(), (*is_occupied).to_string()),
                    ]),
                });
                id += 1;
            }
        }
    }
    BlockRegistry::from_report(&[
        BlockReport {
            id: Identifier::parse("minecraft:air").unwrap(),
            properties: BTreeMap::new(),
            states: vec![BlockStateReport {
                id: 0,
                default: true,
                properties: BTreeMap::new(),
            }],
        },
        BlockReport {
            id: Identifier::parse("minecraft:white_bed").unwrap(),
            properties: BTreeMap::from([
                (
                    "facing".to_string(),
                    facing.iter().map(|v| v.to_string()).collect(),
                ),
                (
                    "part".to_string(),
                    part.iter().map(|v| v.to_string()).collect(),
                ),
                (
                    "occupied".to_string(),
                    occupied.iter().map(|v| v.to_string()).collect(),
                ),
            ]),
            states,
        },
    ])
    .unwrap()
}

#[test]
fn client_block_entities_synthesize_bed_records_from_block_states() {
    let registry = air_bed_registry();
    let items = item_registry();
    let bed_state = |part: &str| {
        registry
            .by_name_and_props(
                &Identifier::parse("minecraft:white_bed").unwrap(),
                &[
                    ("facing".to_string(), "north".to_string()),
                    ("part".to_string(), part.to_string()),
                    ("occupied".to_string(), "false".to_string()),
                ],
            )
            .expect("bed state resolves")
    };
    let mut chunk = empty_chunk();
    let foot_pos = BlockPos { x: 2, y: 64, z: 3 };
    let head_pos = BlockPos { x: 2, y: 64, z: 2 };
    chunk.set_block(2, 64, 3, bed_state("foot")).unwrap();
    chunk.set_block(2, 64, 2, bed_state("head")).unwrap();

    let records = client_block_entities(&chunk, &registry, &items);

    let mut positions: Vec<_> = records.iter().map(|record| record.pos).collect();
    positions.sort_by_key(|pos| (pos.x, pos.y, pos.z));
    assert_eq!(positions, vec![head_pos, foot_pos]);
    assert!(
        records
            .iter()
            .all(|record| record.type_name.as_str() == "minecraft:bed")
    );
    // Vanilla's bed block entity carries no payload: the empty update tag
    // is the whole record, the block state carries part/facing/colour.
    assert!(
        records
            .iter()
            .all(|record| record.nbt == Tag::Compound(Vec::new()))
    );
}

#[test]
fn stored_bed_block_entities_are_not_duplicated_by_synthesis() {
    let registry = air_bed_registry();
    let items = item_registry();
    let bed_state = |part: &str| {
        registry
            .by_name_and_props(
                &Identifier::parse("minecraft:white_bed").unwrap(),
                &[
                    ("facing".to_string(), "north".to_string()),
                    ("part".to_string(), part.to_string()),
                    ("occupied".to_string(), "false".to_string()),
                ],
            )
            .expect("bed state resolves")
    };
    let mut chunk = empty_chunk();
    let foot_pos = BlockPos { x: 2, y: 64, z: 3 };
    let _head_pos = BlockPos { x: 2, y: 64, z: 2 };
    chunk.set_block(2, 64, 3, bed_state("foot")).unwrap();
    chunk.set_block(2, 64, 2, bed_state("head")).unwrap();
    let tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:bed".into())),
        ("x".into(), Tag::Int(foot_pos.x)),
        ("y".into(), Tag::Int(foot_pos.y)),
        ("z".into(), Tag::Int(foot_pos.z)),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).expect("encode bed block entity");
    chunk.block_entities.insert(foot_pos, bytes);

    let records = client_block_entities(&chunk, &registry, &items);

    assert_eq!(records.len(), 2);
    assert_eq!(
        records
            .iter()
            .filter(|record| record.pos == foot_pos)
            .count(),
        1,
        "the stored bed record must not be emitted twice"
    );
}

#[test]
fn client_block_entities_leave_bedless_chunks_empty() {
    let chunk = empty_chunk();
    let records = client_block_entities(&chunk, &air_bed_registry(), &item_registry());
    assert!(records.is_empty());
}

#[test]
fn single_non_air_block_picks_up_promoted_palette() {
    // Place one bedrock at the section-local origin of section 0.
    let mut chunk = empty_chunk();
    chunk.set_block(0, -64, 0, BEDROCK).unwrap();

    let bytes = encode_chunk_data(&chunk, &biome_registry(), &air_chest_registry()).unwrap();

    // Section 0 is now indirect:
    //   i16 non_air = 1, i16 fluid = 0          (4 bytes)
    //   u8 bpe = 4                              (1 byte)
    //   VarInt palette_len = 2                  (1 byte)
    //   VarInt(AIR = 0)                         (1 byte)
    //   VarInt(BEDROCK = 74)                    (1 byte)
    //   long[256]: 4096 entries × 4 bits / 64 bits_per_word
    //              = 256 words × 8 bytes_per_word
    //                                          (2048 bytes)
    //   Biome single-value: u8(0) + VarInt(plains = 1)
    //                                           (2 bytes)
    let block_long_bytes = 256 * 8;
    let want_len = 4 + 1 + 1 + 1 + 1 + block_long_bytes + 2;

    assert_eq!(&bytes[..4], &[0x00, 0x01, 0x00, 0x00]); // counts
    assert_eq!(bytes[4], 4, "blocks bpe should be 4");
    assert_eq!(bytes[5], 2, "palette size = 2");
    assert_eq!(bytes[6], 0, "palette[0] = AIR (state 0)");
    // BEDROCK=74 is 0x4A, fits in one VarInt byte.
    assert_eq!(bytes[7], 0x4A);

    // The first 4-bit entry in the packed array must be palette
    // index 1 (BEDROCK). In vanilla's "non-crossing" layout the
    // first entry occupies bits 0..3 of word 0.
    let word0 = i64::from_be_bytes(bytes[8..16].try_into().unwrap()) as u64;
    assert_eq!(word0 & 0xF, 1, "cell (0,0,0) maps to palette[1]");

    // Biomes still single-value.
    let biome_off = 4 + 1 + 1 + 1 + 1 + block_long_bytes;
    assert_eq!(bytes[biome_off], 0, "biome bpe = 0");
    assert_eq!(bytes[biome_off + 1], 1, "biome = plains (index 1)");

    // Sections 1..24 are still single-value air + plains
    // (8 bytes each, see `empty_chunk_emits_single_value_air_sections`).
    assert_eq!(bytes.len(), want_len + 23 * 8);
}

// ---- client_heightmaps ordering ----

#[test]
fn client_heightmaps_skip_worldgen_only_kinds() {
    let mut chunk = empty_chunk();
    // Worldgen-only heightmap that the client should NOT see.
    chunk
        .heightmaps
        .insert("OCEAN_FLOOR_WG".into(), Heightmap::zeroed());
    // Two client-visible heightmaps in arbitrary insertion order.
    chunk
        .heightmaps
        .insert("MOTION_BLOCKING".into(), Heightmap::zeroed());
    chunk
        .heightmaps
        .insert("WORLD_SURFACE".into(), Heightmap::zeroed());

    let entries = client_heightmaps(&chunk);
    assert_eq!(entries.len(), 2, "WG-only key excluded");
    // Output is in WORLD_SURFACE, MOTION_BLOCKING, … order
    // regardless of HashMap insertion order.
    assert_eq!(entries[0].type_id, heightmap_type::WORLD_SURFACE);
    assert_eq!(entries[1].type_id, heightmap_type::MOTION_BLOCKING);
}

#[test]
fn client_heightmaps_passes_long_array_through_unchanged() {
    let mut chunk = empty_chunk();
    let mut hm = Heightmap::zeroed();
    hm.set(3, 5, 42);
    let expected = hm.to_long_array();
    chunk.heightmaps.insert("MOTION_BLOCKING".into(), hm);

    let entries = client_heightmaps(&chunk);
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].data, expected);
}

// ---- error cases ----

#[test]
fn unknown_biome_surfaces_as_error() {
    let chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        AIR,
        Identifier::parse("minecraft:nope").unwrap(),
    );
    let err = encode_chunk_data(&chunk, &biome_registry(), &air_chest_registry()).unwrap_err();
    assert_eq!(
        err,
        WireError::UnknownBiome(Identifier::parse("minecraft:nope").unwrap())
    );
}

// ---- VarInt helper sanity ----

#[test]
fn write_varint_matches_known_encodings() {
    let cases: &[(i32, &[u8])] = &[
        (0, &[0x00]),
        (1, &[0x01]),
        (127, &[0x7F]),
        (128, &[0x80, 0x01]),
        (255, &[0xFF, 0x01]),
        (25565, &[0xDD, 0xC7, 0x01]),
        (-1, &[0xFF, 0xFF, 0xFF, 0xFF, 0x0F]),
    ];
    for (value, expected) in cases {
        let mut buf = Vec::new();
        write_varint(&mut buf, *value);
        assert_eq!(&buf, expected, "VarInt({value})");
    }
}

#[test]
fn encodes_zero_chunk_as_all_empty_with_open_top() {
    let light = ChunkLight::zeroed();
    let wire = encode_chunk_light(&light);

    // Slot 25 (above world) is the only present sky slot;
    // everything else is empty in the sky channel.
    assert_eq!(wire.sky_y_mask.len(), 1);
    assert_eq!(wire.sky_y_mask[0], 1 << 25);
    assert_eq!(wire.sky_updates.len(), 1);
    assert!(wire.sky_updates[0].iter().all(|&b| b == 0xFF));
    assert_eq!(wire.empty_sky_y_mask.len(), 1);
    // All 26 slots except slot 25 are empty.
    let expected_empty_sky = ((1u64 << WIRE_LIGHT_SECTIONS) - 1) & !(1 << 25);
    assert_eq!(wire.empty_sky_y_mask[0], expected_empty_sky as i64);

    // Block channel: all 26 slots empty, no layers shipped.
    assert!(wire.block_updates.is_empty());
    assert_eq!(wire.block_y_mask[0], 0);
    let expected_empty_block = (1u64 << WIRE_LIGHT_SECTIONS) - 1;
    assert_eq!(wire.empty_block_y_mask[0], expected_empty_block as i64);
}

#[test]
fn nibble_packing_round_trips_low_first() {
    // Build a ChunkLight whose section 0 has cell index 0 = 1,
    // cell index 1 = 2, both in the layer's first byte. Then
    // verify pack_section_layer puts them at low (0x01) and
    // high (0x20) nibbles → 0x21 in byte 0.
    let mut light = ChunkLight::zeroed();
    // cell 0 in section 0 = (sub_y=0, z=0, x=0).
    light.set_sky_local(0, 0, 0, 1);
    // cell 1 in section 0 = (sub_y=0, z=0, x=1).
    light.set_sky_local(1, 0, 0, 2);
    let layer = pack_section_layer(&light.sky, 0).unwrap();
    assert_eq!(layer[0], 0x21);
    // Other bytes are untouched.
    assert!(layer[1..].iter().all(|&b| b == 0));
}

#[test]
fn pack_layer_sees_each_section_independently() {
    // Place value 15 at the very last cell of section 0
    // (sub_y=15, z=15, x=15) and value 7 at the very first cell
    // of section 1 (sub_y=0, z=0, x=0). pack_section_layer(_, 0)
    // must see only the 15; pack_section_layer(_, 1) only the 7.
    let mut light = ChunkLight::zeroed();
    // Section 0, sub_y=15 → chunk-light local_y = 15.
    light.set_sky_local(15, 15, 15, 15);
    // Section 1, sub_y=0 → chunk-light local_y = 16.
    light.set_sky_local(0, 16, 0, 7);

    let s0 = pack_section_layer(&light.sky, 0).unwrap();
    let s1 = pack_section_layer(&light.sky, 1).unwrap();
    // Section 0: last cell is index 4095 in the section, byte 2047,
    // high nibble.
    assert_eq!(s0[2047], 0xF0);
    let zeros_in_s0: usize = s0[..2047].iter().filter(|&&b| b == 0).count();
    assert_eq!(zeros_in_s0, 2047);
    // Section 1: first cell is index 0, byte 0, low nibble = 7.
    assert_eq!(s1[0], 0x07);
    assert!(s1[1..].iter().all(|&b| b == 0));
}

#[test]
fn encodes_non_empty_block_section_into_present_mask() {
    // Drop a single glowstone-equivalent cell into section 4
    // (around world Y=0). Expect: section 4 + 1 = wire slot 5 in
    // the present mask; slot 0 + slots 1..=4 + slots 6..=24 +
    // slot 25 in the empty mask; one layer in block_updates.
    let mut light = ChunkLight::zeroed();
    light.set_block_local(8, 64, 8, 15); // section idx = 64/16 = 4
    let wire = encode_chunk_light(&light);

    assert_eq!(wire.block_updates.len(), 1, "single non-zero section");
    assert_eq!(wire.block_y_mask[0], 1 << 5);

    let layer = &wire.block_updates[0];
    // Cell index within section 4: sub_y=0, z=8, x=8 →
    // 0 * 256 + 8 * 16 + 8 = 136. Byte 68, low nibble (136 even).
    assert_eq!(layer[68], 0x0F);
    let nonzero_bytes: usize = layer.iter().filter(|&&b| b != 0).count();
    assert_eq!(nonzero_bytes, 1, "only one cell lit");
}

#[test]
fn block_palette_switches_to_direct_mode_past_256_entries() {
    // M5.c.3: build a section with 260 distinct synthetic states
    // and ensure the wire encoder emits the GlobalPalette
    // (direct) shape — bits_per_entry first byte = DIRECT_BITS,
    // VarInt palette length absent, raw packed long array follows.
    use crate::section::ChunkSection;
    let mut section = ChunkSection::filled(AIR, AIR);
    for i in 1..=260u32 {
        let cell = i - 1;
        let x = (cell & 0x0F) as u8;
        let y = ((cell >> 4) & 0x0F) as u8;
        let z = ((cell >> 8) & 0x0F) as u8;
        section.set(x, y, z, BlockStateId(i));
    }

    let mut buf = Vec::new();
    encode_block_palette(&mut buf, &section, &|state| state);

    assert_eq!(buf[0], DIRECT_BITS, "first byte = direct bits");
    // No VarInt palette length: the next bytes are the packed
    // long array. Compute the expected long count.
    let bits = DIRECT_BITS as usize;
    let expected_longs = SECTION_VOLUME.div_ceil(64 / bits);
    assert_eq!(
        buf.len() - 1,
        expected_longs * 8,
        "direct-mode body = {expected_longs} longs (no palette section)"
    );
}

#[test]
fn block_palette_uses_vanilla_fixed_longs_for_five_bit_indirect() {
    use crate::section::ChunkSection;
    let mut section = ChunkSection::filled(AIR, AIR);
    for i in 1..=17u32 {
        let cell = i - 1;
        section.set(
            (cell & 0x0F) as u8,
            ((cell >> 4) & 0x0F) as u8,
            0,
            BlockStateId(i),
        );
    }

    let mut buf = Vec::new();
    encode_block_palette(&mut buf, &section, &|state| state);

    assert_eq!(buf[0], 5, "17 states require five bits");
    let mut offset = 1;
    assert_eq!(buf[offset], 18, "palette includes air plus 17 states");
    offset += 1;
    for state in 0..=17u8 {
        assert_eq!(buf[offset], state);
        offset += 1;
    }
    let expected_longs = SECTION_VOLUME.div_ceil(64 / 5);
    assert_eq!(buf.len() - offset, expected_longs * 8);

    let word0 = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap());
    let word1 = u64::from_be_bytes(buf[offset + 8..offset + 16].try_into().unwrap());
    assert_eq!(word0 >> 60, 0, "top four bits are padding");
    assert_eq!(word1 & 0x1F, 13, "entry 12 starts a fresh word");
}

#[test]
fn biome_palette_uses_vanilla_fixed_longs_for_three_bit_indirect() {
    let registry = numbered_biome_registry(8);
    let palette: Vec<_> = registry.entries[..5].to_vec();
    let mut indices = PackedBitArray::zeroed(3, BIOME_VOLUME);
    for idx in 0..BIOME_VOLUME {
        indices.set(idx, (idx % palette.len()) as u32);
    }
    let section = BiomeSection::from_indirect(palette, indices);

    let mut buf = Vec::new();
    encode_biome_palette(&mut buf, &section, &registry).unwrap();

    assert_eq!(buf[0], 3, "five biomes stay indirect at three bits");
    let mut offset = 1;
    assert_eq!(buf[offset], 5, "palette length");
    offset += 1 + 5;
    let expected_longs = BIOME_VOLUME.div_ceil(64 / 3);
    assert_eq!(buf.len() - offset, expected_longs * 8);

    let word0 = u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap());
    let word1 = u64::from_be_bytes(buf[offset + 8..offset + 16].try_into().unwrap());
    assert_eq!(word0 >> 63, 0, "top bit is padding");
    assert_eq!(word1 & 0x07, 1, "entry 21 starts a fresh word");
}

#[test]
fn biome_palette_switches_to_direct_mode_above_three_bits() {
    let registry = numbered_biome_registry(10);
    let palette = registry.entries.clone();
    let mut indices = PackedBitArray::zeroed(4, BIOME_VOLUME);
    for idx in 0..BIOME_VOLUME {
        indices.set(idx, (idx % palette.len()) as u32);
    }
    let section = BiomeSection::from_indirect(palette, indices);

    let mut buf = Vec::new();
    encode_biome_palette(&mut buf, &section, &registry).unwrap();

    assert_eq!(buf[0], 4, "ten-entry biome registry needs four direct bits");
    let expected_longs = BIOME_VOLUME.div_ceil(64 / 4);
    assert_eq!(
        buf.len() - 1,
        expected_longs * 8,
        "direct biome body has no palette length or entries"
    );
}

#[test]
fn block_palette_stays_indirect_for_small_palettes() {
    // Sanity check that the M3.b indirect path didn't regress —
    // a 3-state section still uses the LinearPalette / 4-bit form.
    use crate::section::ChunkSection;
    let mut section = ChunkSection::filled(AIR, AIR);
    section.set(0, 0, 0, BlockStateId(1));
    section.set(1, 0, 0, BlockStateId(2));

    let mut buf = Vec::new();
    encode_block_palette(&mut buf, &section, &|state| state);

    assert!(
        buf[0] < DIRECT_BITS_THRESHOLD,
        "small palette uses indirect bpe; got {}",
        buf[0],
    );
}
