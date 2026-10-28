use super::*;
use crate::Identifier;
use crate::blocks::{BlockReport, BlockStateReport};
use std::collections::BTreeMap;
use std::path::PathBuf;
use tempfile::TempDir;

fn workspace_path(rel: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join(rel)
}

fn write_json(dir: &TempDir, name: &str, body: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, body).unwrap();
    path
}

fn block(id: u32, name: &str) -> BlockReport {
    BlockReport {
        id: Identifier::parse(name).unwrap(),
        properties: BTreeMap::new(),
        states: vec![BlockStateReport {
            id,
            default: true,
            properties: BTreeMap::new(),
        }],
    }
}

#[test]
fn from_arrays_pins_lookups() {
    let mut table = BlockLightTable::from_arrays(
        "test",
        vec![0, 14, 15],
        vec![0, 0, 15],
        vec![true, true, false],
    );
    assert_eq!(table.len(), 3);
    assert!(!table.is_empty());
    assert_eq!(table.emission(0), Some(0));
    assert_eq!(table.opacity(2), Some(15));
    assert_eq!(table.propagates_sky(1), Some(true));
    assert_eq!(table.suffocating(2), Some(true));
    assert_eq!(table.emission(99), None);
    table.append_opaque_state();
    assert_eq!(table.emission(3), Some(0));
    assert_eq!(table.opacity(3), Some(15));
    assert_eq!(table.propagates_sky(3), Some(false));
    assert_eq!(table.suffocating(3), Some(true));
}

#[test]
fn conservative_table_from_blocks_report_keeps_light_running_without_sidecar() {
    let table = BlockLightTable::conservative_from_blocks_report(&[
        block(0, "minecraft:air"),
        block(1, "minecraft:stone"),
        block(2, "minecraft:grass_block"),
        block(3, "minecraft:glass"),
        block(4, "minecraft:torch"),
        block(5, "minecraft:glowstone"),
        block(6, "minecraft:poppy"),
        block(7, "minecraft:sugar_cane"),
        block(8, "minecraft:wheat"),
    ]);

    assert_eq!(table.version, "blocks-report-conservative");
    assert_eq!(table.len(), 9);
    assert_eq!(table.opacity(0), Some(0));
    assert_eq!(table.propagates_sky(0), Some(true));
    assert_eq!(table.opacity(1), Some(15));
    assert_eq!(table.propagates_sky(1), Some(false));
    assert_eq!(
        table.opacity(2),
        Some(15),
        "grass_block is terrain, not grass"
    );
    assert_eq!(table.opacity(3), Some(0));
    assert_eq!(table.emission(4), Some(14));
    assert_eq!(table.emission(5), Some(15));
    assert_eq!(table.opacity(6), Some(0));
    assert_eq!(table.opacity(7), Some(0));
    assert_eq!(table.opacity(8), Some(0));
    assert_eq!(table.propagates_sky(8), Some(true));
}

#[test]
fn conservative_table_matches_supported_crop_light_classes() {
    let full_sky = [
        "minecraft:wheat",
        "minecraft:carrots",
        "minecraft:potatoes",
        "minecraft:beetroots",
        "minecraft:torchflower_crop",
        "minecraft:torchflower",
        "minecraft:pitcher_crop",
        "minecraft:pitcher_plant",
        "minecraft:melon_stem",
        "minecraft:attached_melon_stem",
        "minecraft:pumpkin_stem",
        "minecraft:attached_pumpkin_stem",
        "minecraft:sweet_berry_bush",
        "minecraft:nether_wart",
        "minecraft:cocoa",
        "minecraft:cactus",
        "minecraft:sugar_cane",
        "minecraft:bamboo",
        "minecraft:bamboo_sapling",
    ];
    let soft_light = [
        "minecraft:kelp",
        "minecraft:kelp_plant",
        "minecraft:chorus_flower",
        "minecraft:chorus_plant",
    ];
    let reports = full_sky
        .iter()
        .chain(soft_light.iter())
        .enumerate()
        .map(|(id, name)| block(id as u32, name))
        .collect::<Vec<_>>();
    let table = BlockLightTable::conservative_from_blocks_report(&reports);

    for (id, name) in full_sky.iter().enumerate() {
        assert_eq!(table.emission(id as u32), Some(0), "{name}");
        assert_eq!(table.opacity(id as u32), Some(0), "{name}");
        assert_eq!(table.propagates_sky(id as u32), Some(true), "{name}");
    }
    for (offset, name) in soft_light.iter().enumerate() {
        let id = (full_sky.len() + offset) as u32;
        assert_eq!(table.emission(id), Some(0), "{name}");
        assert_eq!(table.opacity(id), Some(1), "{name}");
        assert_eq!(table.propagates_sky(id), Some(false), "{name}");
    }
}

#[test]
fn conservative_table_matches_glow_berry_vine_emission() {
    let mut berries = BTreeMap::new();
    berries.insert("berries".to_string(), "true".to_string());
    let mut empty = BTreeMap::new();
    empty.insert("berries".to_string(), "false".to_string());

    assert_eq!(conservative_emission("cave_vines", &berries), 14);
    assert_eq!(conservative_emission("cave_vines", &empty), 0);
    assert_eq!(conservative_emission("cave_vines_plant", &berries), 14);
    assert_eq!(conservative_emission("cave_vines_plant", &empty), 0);
    assert_eq!(conservative_opacity("cave_vines"), 0);
    assert_eq!(conservative_opacity("cave_vines_plant"), 0);
}

#[test]
fn conservative_chest_opacity_follows_waterlogged_state() {
    let state = |id: u32, waterlogged: &str| BlockStateReport {
        id,
        default: waterlogged == "false",
        properties: BTreeMap::from([("waterlogged".to_string(), waterlogged.to_string())]),
    };
    let report = vec![BlockReport {
        id: Identifier::parse("minecraft:chest").unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(0, "false"), state(1, "true")],
    }];
    let table = BlockLightTable::conservative_from_blocks_report(&report);
    assert_eq!(table.opacity[0], 0);
    assert_eq!(table.opacity[1], 1);
    assert!(table.propagates_sky[0]);
    assert!(!table.propagates_sky[1]);
    assert!(!table.suffocating[0]);
    assert!(!table.suffocating[1]);
}
#[test]
fn conservative_ender_chest_emission_is_seven() {
    let state = |id: u32, waterlogged: &str| BlockStateReport {
        id,
        default: waterlogged == "false",
        properties: BTreeMap::from([("waterlogged".to_string(), waterlogged.to_string())]),
    };
    let report = vec![BlockReport {
        id: Identifier::parse("minecraft:ender_chest").unwrap(),
        properties: BTreeMap::new(),
        states: vec![state(0, "false"), state(1, "true")],
    }];
    let table = BlockLightTable::conservative_from_blocks_report(&report);
    // Vanilla 26.1.2 block_light.json: every ender_chest state is [7, ..].
    assert_eq!(table.emission[0], 7);
    assert_eq!(table.emission[1], 7);
    // Opacity still follows the chest waterlogged rule, not 15.
    assert_eq!(table.opacity[0], 0);
    assert_eq!(table.opacity[1], 1);
}
#[test]
fn loads_synthetic_table() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "block_light.json",
        r#"{
            "version": "26.1.2-test",
            "max_state_id": 2,
            "entries": [[0,0,1,0],[14,0,1,0],[0,15,0,1]]
        }"#,
    );
    let t = load(&path).unwrap();
    assert_eq!(t.version, "26.1.2-test");
    assert_eq!(t.len(), 3);
    assert_eq!(t.emission(1), Some(14));
    assert_eq!(t.opacity(2), Some(15));
    assert_eq!(t.propagates_sky(2), Some(false));
}

#[test]
fn loads_synthetic_table_with_suffocation_fact() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "block_light.json",
        r#"{
            "version": "26.1.2-test",
            "max_state_id": 1,
            "entries": [[0,0,1,0],[0,15,0,1]]
        }"#,
    );

    let table = load(&path).expect("four-field block-state facts should load");
    assert_eq!(table.len(), 2);
    assert_eq!(table.suffocating(0), Some(false));
    assert_eq!(table.suffocating(1), Some(true));
}

#[test]
fn three_field_table_requires_regeneration() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "block_light.json",
        r#"{
            "version": "26.1.2-test",
            "max_state_id": 0,
            "entries": [[0,0,1]]
        }"#,
    );

    let error = load(&path).expect_err("old table must not guess suffocation from opacity");
    assert!(matches!(
        error,
        BlockLightError::EntryShape {
            state_id: 0,
            got: 3
        }
    ));
}

#[test]
fn missing_file_is_reported_clearly() {
    let err = load("/definitely/does/not/exist/block_light.json").unwrap_err();
    assert!(matches!(err, BlockLightError::Missing(_)));
}

#[test]
fn length_mismatch_is_an_error() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "x.json",
        r#"{"version":"v","max_state_id":1,"entries":[[0,0,1]]}"#,
    );
    let err = load(&path).unwrap_err();
    assert!(matches!(err, BlockLightError::LengthMismatch { .. }));
}

#[test]
fn out_of_range_emission_is_an_error() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "x.json",
        r#"{"version":"v","max_state_id":0,"entries":[[16,0,1,0]]}"#,
    );
    let err = load(&path).unwrap_err();
    assert!(matches!(
        err,
        BlockLightError::OutOfRange {
            field: "emission",
            value: 16,
            ..
        }
    ));
}

#[test]
fn invalid_propagates_sky_is_an_error() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "x.json",
        r#"{"version":"v","max_state_id":0,"entries":[[0,0,2,0]]}"#,
    );
    let err = load(&path).unwrap_err();
    assert!(matches!(err, BlockLightError::InvalidBool { value: 2, .. }));
}

#[test]
fn entry_shape_is_an_error() {
    let dir = TempDir::new().unwrap();
    let path = write_json(
        &dir,
        "x.json",
        r#"{"version":"v","max_state_id":0,"entries":[[0,0]]}"#,
    );
    let err = load(&path).unwrap_err();
    assert!(matches!(
        err,
        BlockLightError::EntryShape {
            state_id: 0,
            got: 2
        }
    ));
}

/// Pin a handful of values against the real extracted JSON when
/// it is present. The exact state-ids are resolved through
/// `blocks.json` so the test survives state-id reshuffles
/// between Mojang patches.
#[test]
#[ignore = "requires local 26.1.2 block light and blocks reports"]
fn real_table_matches_known_blocks() {
    let blocks_path = workspace_path("data/vanilla/reports/blocks.json");
    let light_path = workspace_path("data/vanilla/reports/block_light.json");
    assert!(
        blocks_path.is_file() && light_path.is_file(),
        "need both {} and {}; run tools/extract-vanilla-data.sh",
        blocks_path.display(),
        light_path.display()
    );

    let blocks = crate::blocks::load_blocks_report(&blocks_path).unwrap();
    let table = load(&light_path).unwrap();
    assert_eq!(
        table.version, "26.3",
        "block_light.json version should track 26.1.2",
    );
    assert_eq!(table.len(), 35723, "26.3 has 35723 block states");

    let default_state = |name: &str| -> u32 {
        blocks
            .iter()
            .find(|b| b.id.as_str() == name)
            .unwrap_or_else(|| panic!("missing {name}"))
            .states
            .iter()
            .find(|s| s.default)
            .unwrap_or_else(|| panic!("{name} has no default state"))
            .id
    };

    // air: transparent, no emission, sky passes through.
    let air = default_state("minecraft:air");
    assert_eq!(table.emission(air), Some(0));
    assert_eq!(table.opacity(air), Some(0));
    assert_eq!(table.propagates_sky(air), Some(true));

    // stone: opaque, no emission, blocks sky.
    let stone = default_state("minecraft:stone");
    assert_eq!(table.emission(stone), Some(0));
    assert_eq!(table.opacity(stone), Some(15));
    assert_eq!(table.propagates_sky(stone), Some(false));
    assert_eq!(table.suffocating(stone), Some(true));

    let slab = default_state("minecraft:oak_slab");
    assert_eq!(table.suffocating(slab), Some(false));

    let soul_sand = default_state("minecraft:soul_sand");
    assert_eq!(table.suffocating(soul_sand), Some(true));

    let barrier = default_state("minecraft:barrier");
    assert_eq!(table.opacity(barrier), Some(0));
    assert_eq!(table.suffocating(barrier), Some(true));

    // glowstone: full luminance (15).
    let glowstone = default_state("minecraft:glowstone");
    assert_eq!(table.emission(glowstone), Some(15));

    // wall_torch: torches sit at emission 14.
    let torch = default_state("minecraft:torch");
    assert_eq!(table.emission(torch), Some(14));

    // soul_lantern: 10 in 26.1.2 (vanilla-known).
    let soul_lantern = default_state("minecraft:soul_lantern");
    assert_eq!(table.emission(soul_lantern), Some(10));

    // ender_chest: 7 in 26.1.2 (vanilla-known; anchors the conservative fallback).
    let ender_chest = default_state("minecraft:ender_chest");
    assert_eq!(table.emission(ender_chest), Some(7));
    // water: opacity 1 (soft attenuator), no emission, sky still
    // passes through (water doesn't fully block sky-light).
    let water = default_state("minecraft:water");
    assert_eq!(table.emission(water), Some(0));
    assert_eq!(table.opacity(water), Some(1));
}

#[test]
fn local_loader_matches_checkout_report() {
    let table = super::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/vanilla/reports/block_light.json"
    ))
    .expect("extracted vanilla report loads");
    assert!(!table.is_empty());
    assert_eq!(table.version, "26.3");
}
