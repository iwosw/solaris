use super::*;
use mc_physics::{Aabb, BlockCollisionBox, BlockSampler, Vec3, resolve_collision_displacement};

#[test]
fn dense_boxes_match_every_recorded_vanilla_state() {
    let registry = CoreRegistry::load(None).expect("registry");
    let shapes = mc_data::collision_shapes::vanilla_collision_shapes();
    for state in registry.blocks.states() {
        let Some(shape) = shapes.get_for_state(state.id.0, &state.block.id, &state.properties)
        else {
            continue;
        };
        let expected: Vec<_> = shape.iter().map(|b| b.coordinates()).collect();
        let block_at = |_: i32, _: i32, _: i32| Some(state.id.0);
        let sampler = WorkerSampler {
            block_at: &block_at,
            registry: &registry,
        };
        let mut actual = Vec::new();
        sampler.collision_boxes_at(0, 0, 0, &mut |b| actual.push(b.coordinates()));
        assert_eq!(
            actual, expected,
            "{} {:?}",
            state.block.id, state.properties
        );
    }
}

#[test]
fn fence_support_is_found_below_body_and_missing_chunks_block() {
    let registry = CoreRegistry::load(None).expect("registry");
    let fence = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:oak_fence").expect("id"))
        .expect("fence")
        .default
        .0;
    let block_at = |x: i32, y: i32, z: i32| {
        Some(if (x, y, z) == (0, 0, 0) {
            fence
        } else {
            registry.states.air
        })
    };
    let sampler = WorkerSampler {
        block_at: &block_at,
        registry: &registry,
    };
    assert!(sampler.max_collision_box_y() >= 24);
    let resolved = resolve_collision_displacement(
        &sampler,
        Vec3::new(0.5, 1.6, 0.5),
        Aabb {
            half_width: 0.3,
            height: 1.8,
        },
        Vec3::new(0.0, -0.2, 0.0),
        false,
        0.0,
    )
    .expect("resolved");
    assert!((1.6 + resolved.delta.y - 1.5).abs() < 1.0e-12);
    assert!(resolved.grounded);
    let absent = |_: i32, _: i32, _: i32| None;
    let sampler = WorkerSampler {
        block_at: &absent,
        registry: &registry,
    };
    let mut boxes = Vec::new();
    sampler.collision_boxes_at(0, 0, 0, &mut |b| boxes.push(b));
    assert_eq!(boxes, [BlockCollisionBox::FULL_BLOCK]);
}
