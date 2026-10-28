use crate::Identifier;
use crate::blocks::solaris_required_blocks_report;

use super::{CollisionBox, vanilla_collision_shapes};

fn box_16(min_x: i16, min_y: i16, min_z: i16, max_x: i16, max_y: i16, max_z: i16) -> CollisionBox {
    CollisionBox::new(
        min_x * 256,
        min_y * 256,
        min_z * 256,
        max_x * 256,
        max_y * 256,
        max_z * 256,
    )
}

fn shape(state_id: u32) -> Vec<CollisionBox> {
    vanilla_collision_shapes()
        .get(state_id)
        .expect("state collision shape")
        .iter()
        .collect()
}

fn state_id(block_name: &str, properties: &[(&str, &str)]) -> u32 {
    let blocks = solaris_required_blocks_report();
    let block = blocks
        .iter()
        .find(|block| block.id.as_str() == block_name)
        .unwrap_or_else(|| panic!("missing block {block_name}"));
    block
        .states
        .iter()
        .find(|state| {
            properties.iter().all(|(name, value)| {
                state.properties.get(*name).map(String::as_str) == Some(*value)
            })
        })
        .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"))
        .id
}

fn state_identity(
    block_name: &str,
    properties: &[(&str, &str)],
) -> (u32, Identifier, Vec<(String, String)>) {
    let blocks = solaris_required_blocks_report();
    let block = blocks
        .iter()
        .find(|block| block.id.as_str() == block_name)
        .unwrap_or_else(|| panic!("missing block {block_name}"));
    let state = block
        .states
        .iter()
        .find(|state| {
            properties.iter().all(|(name, value)| {
                state.properties.get(*name).map(String::as_str) == Some(*value)
            })
        })
        .unwrap_or_else(|| panic!("missing state for {block_name}: {properties:?}"));
    (
        state.id,
        block.id.clone(),
        state
            .properties
            .iter()
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect(),
    )
}

#[test]
fn oracle_table_has_exact_farmland_shape_for_every_moisture_state() {
    let blocks = solaris_required_blocks_report();
    let farmland = blocks
        .iter()
        .find(|block| block.id.as_str() == "minecraft:farmland")
        .unwrap();
    let expected = [box_16(0, 0, 0, 16, 15, 16)];

    assert_eq!(farmland.states.len(), 8);
    for state in &farmland.states {
        assert_eq!(
            shape(state.id),
            expected,
            "farmland state {} must retain the vanilla 15/16 collision top",
            state.id
        );
    }
}

#[test]
fn oracle_table_covers_the_complete_vanilla_state_registry() {
    let table = vanilla_collision_shapes();

    assert_eq!(table.version(), "26.3");
    assert_eq!(table.covered_state_count(), 35_723);
}

#[test]
fn oracle_table_keeps_state_dependent_slab_stair_and_fence_boxes() {
    let bottom_slab = state_id(
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let top_slab = state_id(
        "minecraft:stone_slab",
        &[("type", "top"), ("waterlogged", "false")],
    );
    let straight_stair = state_id(
        "minecraft:oak_stairs",
        &[
            ("facing", "north"),
            ("half", "bottom"),
            ("shape", "straight"),
            ("waterlogged", "false"),
        ],
    );
    let isolated_fence = state_id(
        "minecraft:oak_fence",
        &[
            ("east", "false"),
            ("north", "false"),
            ("south", "false"),
            ("west", "false"),
            ("waterlogged", "false"),
        ],
    );

    assert_eq!(shape(bottom_slab), [box_16(0, 0, 0, 16, 8, 16)]);
    assert_eq!(shape(top_slab), [box_16(0, 8, 0, 16, 16, 16)]);
    assert_eq!(
        shape(straight_stair),
        [box_16(0, 0, 0, 16, 8, 16), box_16(0, 8, 0, 16, 16, 8)]
    );
    assert_eq!(shape(isolated_fence), [box_16(6, 0, 6, 10, 24, 10)]);
}

#[test]
fn oracle_table_keeps_closed_and_open_door_planes() {
    let shapes = vanilla_collision_shapes();
    let closed = state_id(
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "left"),
            ("open", "false"),
            ("powered", "false"),
        ],
    );
    let open = state_id(
        "minecraft:oak_door",
        &[
            ("facing", "north"),
            ("half", "lower"),
            ("hinge", "left"),
            ("open", "true"),
            ("powered", "false"),
        ],
    );

    assert_ne!(shapes.get(closed), shapes.get(open));
    assert!(shapes.get(closed).is_some_and(|boxes| boxes.len() == 1));
    assert!(shapes.get(open).is_some_and(|boxes| boxes.len() == 1));
}

#[test]
fn generated_and_growable_plants_never_use_full_cube_collision() {
    let blocks = solaris_required_blocks_report();
    let table = vanilla_collision_shapes();
    for block_name in [
        "minecraft:short_grass",
        "minecraft:tall_grass",
        "minecraft:fern",
        "minecraft:large_fern",
        "minecraft:dandelion",
        "minecraft:poppy",
        "minecraft:oak_sapling",
        "minecraft:birch_sapling",
        "minecraft:spruce_sapling",
        "minecraft:jungle_sapling",
        "minecraft:acacia_sapling",
        "minecraft:dark_oak_sapling",
        "minecraft:cherry_sapling",
        "minecraft:wheat",
        "minecraft:carrots",
        "minecraft:potatoes",
        "minecraft:beetroots",
        "minecraft:torchflower_crop",
        "minecraft:pitcher_crop",
        "minecraft:pumpkin_stem",
        "minecraft:melon_stem",
    ] {
        let block = blocks
            .iter()
            .find(|block| block.id.as_str() == block_name)
            .unwrap_or_else(|| panic!("missing plant block {block_name}"));
        for state in &block.states {
            let shape = table
                .get_for_state(
                    state.id,
                    &block.id,
                    &state
                        .properties
                        .iter()
                        .map(|(name, value)| (name.clone(), value.clone()))
                        .collect::<Vec<_>>(),
                )
                .unwrap_or_else(|| panic!("missing collision shape for {block_name}"));
            assert!(
                !shape.is_full_cube(),
                "plant {block_name} state {} became a full cube",
                state.id
            );
        }
    }
}

#[test]
fn exact_lookup_accepts_canonical_slab_stair_fence_and_farmland_states() {
    let table = vanilla_collision_shapes();
    for (block_name, properties) in [
        (
            "minecraft:stone_slab",
            &[("type", "bottom"), ("waterlogged", "false")][..],
        ),
        (
            "minecraft:oak_stairs",
            &[
                ("facing", "north"),
                ("half", "bottom"),
                ("shape", "straight"),
                ("waterlogged", "false"),
            ][..],
        ),
        (
            "minecraft:oak_fence",
            &[
                ("east", "false"),
                ("north", "false"),
                ("south", "false"),
                ("west", "false"),
                ("waterlogged", "false"),
            ][..],
        ),
        ("minecraft:farmland", &[("moisture", "0")][..]),
    ] {
        let (state_id, identifier, properties) = state_identity(block_name, properties);
        assert_eq!(
            table.get_for_state(state_id, &identifier, &properties),
            table.get(state_id),
            "canonical state identity must unlock its table shape for {block_name}"
        );
    }
}

#[test]
fn exact_lookup_rejects_overlapping_id_name_and_property_mismatches() {
    let table = vanilla_collision_shapes();
    let (state_id, identifier, properties) = state_identity(
        "minecraft:stone_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    let synthetic_slab = Identifier::parse("minecraft:synthetic_slab").unwrap();
    let mut wrong_properties = properties.clone();
    wrong_properties[0].1 = "synthetic".to_string();
    let mut reordered_properties = properties.clone();
    reordered_properties.reverse();

    assert!(
        table
            .get_for_state(state_id, &synthetic_slab, &properties)
            .is_none()
    );
    assert!(
        table
            .get_for_state(state_id, &identifier, &wrong_properties)
            .is_none()
    );
    assert!(
        table
            .get_for_state(state_id, &identifier, &reordered_properties)
            .is_none()
    );
}

#[test]
fn farmland_fallback_requires_exact_canonical_semantics() {
    let table = vanilla_collision_shapes();
    let (_, farmland, properties) = state_identity("minecraft:farmland", &[("moisture", "0")]);
    let fake_properties = vec![
        ("type".to_string(), "bottom".to_string()),
        ("waterlogged".to_string(), "false".to_string()),
    ];

    assert!(table.is_exact_farmland_state(&farmland, &properties));
    assert!(!table.is_exact_farmland_state(&farmland, &fake_properties));
    assert!(
        !table
            .is_exact_farmland_state(&Identifier::parse("solaris:farmland").unwrap(), &properties,)
    );
}

#[test]
fn oracle_table_reports_maximum_local_collision_y() {
    assert_eq!(vanilla_collision_shapes().max_box_y(), 24 * 256);
}
