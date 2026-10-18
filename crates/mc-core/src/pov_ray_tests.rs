use super::*;
use crate::fluids::Fluid;
use crate::pov_ray::{Hit, clip};

fn use_item(pitch: f32) -> PlayerInput {
    PlayerInput::UseItem {
        connection: ConnectionId(1),
        hand: Hand::Main,
        yaw: 0.0,
        pitch,
        sequence: 41,
    }
}

#[test]
fn pov_ray_exact_java_fire_pose_hits_stone_up_and_bucket_pours_above_stone() {
    let (mut state, actor, _) = setup();
    state.world.ensure_blocking(
        &[(mc_world::ChunkPos { x: 12, z: -29 }, 0)]
            .into_iter()
            .collect(),
    );
    let air = state.registry.states.air;
    for y in 76..=81 {
        for z in -456..=-448 {
            state.world.set_block(192, y, z, air);
        }
    }
    let fire_pos = BlockPos {
        x: 192,
        y: 78,
        z: -453,
    };
    let stone_pos = BlockPos {
        x: 192,
        y: 77,
        z: -452,
    };
    let target = BlockPos { y: 78, ..stone_pos };
    let fire = block(
        &state,
        "fire",
        &[
            ("age", "0"),
            ("north", "false"),
            ("south", "false"),
            ("east", "false"),
            ("west", "false"),
            ("up", "false"),
        ],
    );
    let stone = state.registry.states.stone;
    state
        .world
        .set_block(fire_pos.x, fire_pos.y - 1, fire_pos.z, stone);
    state.world.set_block(192, 76, -455, stone);
    place(&mut state, fire_pos, fire);
    place(&mut state, stone_pos, stone);
    let eye = [192.5, 78.62, -454.404];
    let pitch = 12.0526;
    assert_eq!(
        clip(&state, eye, 0.0, pitch, false),
        Some(Hit {
            pos: stone_pos,
            face: [0, 1, 0],
        })
    );
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, pitch, Some(Fluid::Water)),
        Some((target, state.registry.states.water))
    );
    let slot = state.entities.slot_of(actor).unwrap();
    state
        .entities
        .set_pos(slot, [eye[0], eye[1] - 1.62, eye[2]]);
    let bucket = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(bucket, 1);
    let (out, _, _) = run_tick(&mut state, &[use_item(pitch)]);
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(state.registry.states.water)
    );
    assert_eq!(
        state.world.block_at(fire_pos.x, fire_pos.y, fire_pos.z),
        Some(fire)
    );
    assert!(
        out.changes
            .blocks
            .iter()
            .any(|change| change.position == target)
    );
    assert!(
        !out.changes.blocks.iter().any(
            |change| change.position == fire_pos && change.state == state.registry.states.water
        )
    );
}

#[test]
fn pov_ray_torch_and_short_grass_hit_only_their_selection_boxes() {
    let (mut state, _, pos) = setup();
    let source = BlockPos {
        z: pos.z + 1,
        ..pos
    };
    let water = state.registry.states.water;
    place(&mut state, source, water);
    for name in ["torch", "short_grass", "fern"] {
        let plant = block(&state, name, &[]);
        place(&mut state, pos, plant);
        let eye = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) - 1.5,
        ];
        assert_eq!(
            crate::fluids::bucket_target(&state, eye, 0.0, 0.0, None),
            None,
            "{name} outline occludes pickup"
        );
        let grazing = [f64::from(pos.x) + 0.02, eye[1], eye[2]];
        assert_eq!(
            crate::fluids::bucket_target(&state, grazing, 0.0, 0.0, None),
            Some((source, state.registry.states.air)),
            "{name} voxel outside its outline must not occlude"
        );
    }
}

#[test]
fn pov_ray_tall_grass_outline_occludes_even_though_collision_is_empty() {
    let (mut state, _, pos) = setup();
    let source = BlockPos {
        z: pos.z + 1,
        ..pos
    };
    let water = state.registry.states.water;
    place(&mut state, source, water);
    let grass = block(&state, "tall_grass", &[("half", "lower")]);
    place(&mut state, pos, grass);
    assert!(state.registry.collision_boxes[grass as usize].is_empty());
    let eye = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5,
        f64::from(pos.z) - 1.5,
    ];
    assert_eq!(clip(&state, eye, 0.0, 0.0, true).unwrap().pos, pos);
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 0.0, None),
        None
    );
}

#[test]
fn pov_ray_flower_selection_reapplies_position_hash_not_visual_grass_offset() {
    let (mut state, _, _) = setup();
    state.world.ensure_blocking(
        &[
            (mc_world::ChunkPos { x: 0, z: -1 }, 0),
            (mc_world::ChunkPos { x: 0, z: 0 }, 0),
        ]
        .into_iter()
        .collect(),
    );
    let pos = BlockPos {
        x: 0,
        y: Y + 1,
        z: 0,
    };
    let source = BlockPos { z: 1, ..pos };
    let water = state.registry.states.water;
    place(&mut state, source, water);
    let flower = block(&state, "dandelion", &[]);
    place(&mut state, pos, flower);
    // ZERO's XZ seed is zero: flowers move -0.25 on both horizontal axes.
    let eye = [0.25, f64::from(pos.y) + 0.25, -1.5];
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 0.0, None),
        None
    );
    let grazing = [0.5, eye[1], eye[2]];
    assert_eq!(
        crate::fluids::bucket_target(&state, grazing, 0.0, 0.0, None),
        Some((source, state.registry.states.air))
    );
}

#[test]
fn pov_ray_source_fluid_surface_clips_at_eight_ninths_until_same_fluid_above() {
    let (mut state, _, pos) = setup();
    let water = state.registry.states.water;
    place(&mut state, pos, water);
    let eye = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.95,
        f64::from(pos.z) - 1.5,
    ];
    assert_eq!(clip(&state, eye, 0.0, 0.0, true), None);
    place(
        &mut state,
        BlockPos {
            y: pos.y + 1,
            ..pos
        },
        water,
    );
    assert_eq!(clip(&state, eye, 0.0, 0.0, true).unwrap().pos, pos);
    assert_eq!(clip(&state, eye, 0.0, 0.0, false), None);
}

#[test]
fn pov_ray_partial_block_hit_face_selects_adjacent_cell_not_previous_dda_cell() {
    let (mut state, _, pos) = setup();
    let slab = block(
        &state,
        "oak_slab",
        &[("type", "bottom"), ("waterlogged", "false")],
    );
    place(&mut state, pos, slab);
    let eye = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.9,
        f64::from(pos.z) - 0.2,
    ];
    let hit = clip(&state, eye, 0.0, 20.0, false).unwrap();
    assert_eq!(
        hit,
        Hit {
            pos,
            face: [0, 1, 0]
        }
    );
    let lava = state.registry.states.lava_levels[0];
    assert_eq!(
        crate::fluids::bucket_target(&state, eye, 0.0, 20.0, Some(Fluid::Lava)),
        Some((
            BlockPos {
                y: pos.y + 1,
                ..pos
            },
            lava
        ))
    );
}

#[test]
fn pov_ray_bottle_uses_the_same_outline_occlusion_as_bucket_pickup() {
    let (mut state, actor, pos) = setup();
    let source = BlockPos {
        z: pos.z + 1,
        ..pos
    };
    let water = state.registry.states.water;
    place(&mut state, source, water);
    let torch = block(&state, "torch", &[]);
    place(&mut state, pos, torch);
    let bottle = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:glass_bottle").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = mc_data::ItemStack::new(bottle, 1);
    let slot = state.entities.slot_of(actor).unwrap();
    let base = [
        f64::from(pos.x) + 0.5,
        f64::from(pos.y) + 0.5 - 1.62,
        f64::from(pos.z) - 1.5,
    ];
    state.entities.set_pos(slot, base);
    run_tick(&mut state, &[use_item(0.0)]);
    assert_eq!(state.players[&actor].inventory.slots[0].item_id, bottle);
    state
        .entities
        .set_pos(slot, [f64::from(pos.x) + 0.2, base[1], base[2]]);
    run_tick(&mut state, &[use_item(0.0)]);
    let filled = &state.players[&actor].inventory.slots[0];
    assert!(
        filled
            .extended()
            .potion
            .as_ref()
            .is_some_and(|id| id.as_str() == "minecraft:water")
    );
    assert_eq!(
        state.world.block_at(source.x, source.y, source.z),
        Some(water)
    );
}
