//! Polyfill rules and name-matched id tables over small synthetic reports.

use super::*;

fn reports(packets: &str, registries: &str, blocks: &str) -> Reports {
    Reports {
        packets: serde_json::from_str(packets).unwrap(),
        registries: serde_json::from_str(registries).unwrap(),
        blocks: serde_json::from_str(blocks).unwrap(),
    }
}

#[test]
fn polyfill_rules_pick_nearest_existing_block() {
    assert_eq!(polyfill_block("cinnabar").as_deref(), Some("granite"));
    assert_eq!(
        polyfill_block("poplar_stairs").as_deref(),
        Some("birch_stairs")
    );
    assert_eq!(
        polyfill_block("potted_poplar_sapling").as_deref(),
        Some("potted_birch_sapling")
    );
    assert_eq!(
        polyfill_block("red_concrete_slab").as_deref(),
        Some("red_nether_brick_slab")
    );
    assert_eq!(
        polyfill_block("light_blue_wool_stairs").as_deref(),
        Some("prismarine_stairs")
    );
    assert_eq!(polyfill_block("stone"), None);
    assert_eq!(
        polyfill_item("lime_cushion").as_deref(),
        Some("lime_carpet")
    );
    assert_eq!(polyfill_item("desert_village_map").as_deref(), Some("map"));
    assert_eq!(
        polyfill_item("sulfur_cube_spawn_egg").as_deref(),
        Some("slime_spawn_egg")
    );
    assert_eq!(
        polyfill_item("cinnabar_wall").as_deref(),
        Some("granite_wall")
    );
}

#[test]
fn id_map_matches_names_and_reverses_exact_matches_only() {
    let map = IdMap::by_name(
        [
            ("minecraft:air", 0),
            ("minecraft:cinnabar", 1),
            ("minecraft:stone", 2),
        ],
        [
            ("minecraft:air", 0),
            ("minecraft:stone", 1),
            ("minecraft:granite", 2),
        ],
        polyfill_block,
    );
    assert_eq!(map.get(0), Some(0));
    assert_eq!(map.get(1), Some(2), "cinnabar shown as granite");
    assert_eq!(map.get(2), Some(1), "shifted id follows the name");
    assert_eq!(map.get(3), None);
    assert_eq!(map.back(1), Some(2));
    assert_eq!(map.back(2), None, "granite never becomes cinnabar");
}

#[test]
fn block_states_keep_properties_and_fall_back_to_stone() {
    let canonical = reports(
        "{}",
        "{}",
        r#"{
            "minecraft:stone": {"states": [{"id": 0, "default": true}]},
            "minecraft:cinnabar_slab": {"states": [
                {"id": 1, "properties": {"type": "top", "waterlogged": "true"}},
                {"id": 2, "properties": {"type": "bottom", "waterlogged": "false"}, "default": true}
            ]},
            "minecraft:sulfur_cube_lamp": {"states": [{"id": 3, "default": true}]}
        }"#,
    );
    let legacy = reports(
        "{}",
        "{}",
        r#"{
            "minecraft:granite_slab": {"states": [
                {"id": 7, "properties": {"type": "bottom", "waterlogged": "false"}, "default": true},
                {"id": 8, "properties": {"type": "top", "waterlogged": "true"}}
            ]},
            "minecraft:stone": {"states": [{"id": 5, "default": true}]}
        }"#,
    );
    let map = block_states(&canonical, &legacy).unwrap();
    assert_eq!(map.get(0), Some(5));
    assert_eq!(
        map.get(1),
        Some(8),
        "top waterlogged slab keeps both properties"
    );
    assert_eq!(map.get(2), Some(7));
    assert_eq!(
        map.get(3),
        Some(5),
        "a block with no polyfill becomes stone"
    );
}

#[test]
fn packet_names_pair_ids_across_versions() {
    let canonical = reports(
        r#"{"play": {"clientbound": {
            "minecraft:add_entity": {"protocol_id": 0},
            "minecraft:add_transient_block": {"protocol_id": 1},
            "minecraft:block_update": {"protocol_id": 2}
        }}}"#,
        "{}",
        "{}",
    );
    let legacy = reports(
        r#"{"play": {"clientbound": {
            "minecraft:add_entity": {"protocol_id": 0},
            "minecraft:block_update": {"protocol_id": 1}
        }}}"#,
        "{}",
        "{}",
    );
    let packets = Packets::new(&canonical, &legacy);
    let cb = &packets.play_cb;
    assert_eq!(cb.canonical_name(2), Some("minecraft:block_update"));
    assert_eq!(cb.legacy_id("minecraft:block_update"), Some(1));
    assert_eq!(cb.legacy_id("minecraft:add_transient_block"), None);
    assert_eq!(bits_for(29_873), 15);
    assert_eq!(bits_for(35_723), 16);
    assert_eq!(bits_for(65), 7);
}
