use super::*;
use std::cell::{Cell, RefCell};

fn unit_body() -> WorldAabb {
    WorldAabb {
        min_x: 0.0,
        min_y: 0.0,
        min_z: 0.0,
        max_x: 1.0,
        max_y: 1.0,
        max_z: 1.0,
    }
}

fn wall_x(min_x: f64) -> WorldAabb {
    WorldAabb {
        min_x,
        min_y: -1.0,
        min_z: -10.0,
        max_x: min_x + 1.0,
        max_y: 2.0,
        max_z: 10.0,
    }
}

fn wall_z(min_z: f64) -> WorldAabb {
    WorldAabb {
        min_x: -10.0,
        min_y: -1.0,
        min_z,
        max_x: 10.0,
        max_y: 2.0,
        max_z: min_z + 1.0,
    }
}

#[test]
fn displacement_budget_is_euclidean_and_fails_closed() {
    assert!(displacement_within_limit(Vec3::new(6.0, 8.0, 0.0), 10.0));
    assert!(!displacement_within_limit(
        Vec3::new(6.0, 8.000_001, 0.0),
        10.0,
    ));
    assert!(!displacement_within_limit(
        Vec3::new(f64::NAN, 0.0, 0.0),
        10.0,
    ));
    assert!(!displacement_within_limit(Vec3::ZERO, f64::INFINITY));
    assert!(!displacement_within_limit(Vec3::ZERO, -1.0));
}

#[test]
fn authoritative_pose_limits_accept_edges_and_reject_invalid_values() {
    assert!(authoritative_pose_within_limits(
        Vec3::new(30_000_000.0, 20_000_000.0, -30_000_000.0),
        180.0,
        -90.0,
        30_000_000.0,
        20_000_000.0,
    ));
    assert!(!authoritative_pose_within_limits(
        Vec3::new(30_000_000.001, 0.0, 0.0),
        0.0,
        0.0,
        30_000_000.0,
        20_000_000.0,
    ));
    assert!(!authoritative_pose_within_limits(
        Vec3::ZERO,
        f32::NAN,
        0.0,
        30_000_000.0,
        20_000_000.0,
    ));
    assert!(!authoritative_pose_within_limits(
        Vec3::ZERO,
        0.0,
        f32::INFINITY,
        30_000_000.0,
        20_000_000.0,
    ));
    assert!(!authoritative_pose_within_limits(
        Vec3::ZERO,
        0.0,
        0.0,
        -1.0,
        20_000_000.0,
    ));
}

#[test]
fn sweep_sample_count_bounds_axis_progress_and_rejects_invalid_inputs() {
    assert_eq!(sweep_sample_count(Vec3::ZERO, 1.0 / 32.0), Some(1));
    assert_eq!(
        sweep_sample_count(Vec3::new(1.0, -0.25, 0.5), 1.0 / 32.0),
        Some(32)
    );
    assert_eq!(sweep_sample_count(Vec3::new(1.01, 0.0, 0.0), 0.5), Some(3));
    assert_eq!(sweep_sample_count(Vec3::new(f64::NAN, 0.0, 0.0), 0.5), None);
    assert_eq!(sweep_sample_count(Vec3::ZERO, 0.0), None);
    assert_eq!(sweep_sample_count(Vec3::ZERO, f64::INFINITY), None);
}

#[test]
fn horizontal_movement_residual_matches_vanilla_moved_wrongly_budget() {
    let requested = Vec3::new(0.24, 0.8, -0.24);
    assert!(horizontal_movement_residual_within_limit(
        requested,
        Vec3::new(0.0, -2.0, -0.24),
        0.0625,
    ));
    assert!(!horizontal_movement_residual_within_limit(
        requested,
        Vec3::new(-0.02, 0.8, -0.24),
        0.0625,
    ));
    assert!(!horizontal_movement_residual_within_limit(
        requested,
        Vec3::new(f64::NAN, 0.0, 0.0),
        0.0625,
    ));
    assert!(!horizontal_movement_residual_within_limit(
        requested,
        requested,
        f64::NAN,
    ));
}

#[test]
fn rotation_and_position_normalization_fail_closed() {
    assert!(rotation_is_finite(180.0, -90.0));
    assert!(!rotation_is_finite(f32::NAN, 0.0));
    assert_eq!(
        clamp_world_position(Vec3::new(12.0, -30.0, 40.0), 20.0, 25.0),
        Some(Vec3::new(12.0, -25.0, 20.0))
    );
    assert_eq!(
        clamp_world_position(Vec3::new(f64::NAN, 0.0, 0.0), 20.0, 25.0),
        None
    );
    assert_eq!(clamp_world_position(Vec3::ZERO, -1.0, 25.0), None);
}

#[test]
fn deflated_obstacle_intersection_is_strict_and_fail_closed() {
    let body = [0.0, 0.0, 0.0, 0.6, 1.8, 0.6];
    assert!(aabb_intersects_deflated_obstacle(
        body,
        [0.59, 0.0, 0.0, 1.59, 1.0, 1.0],
        1.0e-5,
    ));
    assert!(!aabb_intersects_deflated_obstacle(
        body,
        [0.6, 0.0, 0.0, 1.6, 1.0, 1.0],
        0.0,
    ));
    assert!(!aabb_intersects_deflated_obstacle(
        body,
        [f64::NAN, 0.0, 0.0, 1.0, 1.0, 1.0],
        0.0,
    ));
    assert!(!aabb_intersects_deflated_obstacle(
        body,
        [1.0, 0.0, 0.0, 0.0, 1.0, 1.0],
        0.0,
    ));
    assert!(!aabb_intersects_deflated_obstacle(
        body,
        [0.0, 0.0, 0.0, 1.0, 1.0, 1.0],
        -1.0,
    ));
}

#[test]
fn fall_start_height_tracks_ground_water_and_airborne_history() {
    assert_eq!(
        next_fall_start_y(12.0, 15.0, false, false, 10.0, true, false),
        Some(10.0)
    );
    assert_eq!(
        next_fall_start_y(12.0, 15.0, false, false, 10.0, false, true),
        Some(10.0)
    );
    assert_eq!(
        next_fall_start_y(12.0, 15.0, true, false, 10.0, false, false),
        Some(12.0)
    );
    assert_eq!(
        next_fall_start_y(12.0, 15.0, false, true, 13.0, false, false),
        Some(13.0)
    );
    assert_eq!(
        next_fall_start_y(12.0, 15.0, false, false, 10.0, false, false),
        Some(15.0)
    );
    assert_eq!(
        next_fall_start_y(f64::NAN, 15.0, false, false, 10.0, false, false),
        None
    );
}

#[test]
fn horizontal_sweep_preserves_partial_x_travel_and_reports_collision() {
    let result = resolve_movement(
        unit_body(),
        Vec3::new(1.0, 0.0, 0.0),
        &[wall_x(1.4)],
        false,
        0.0,
    );

    assert!((result.delta.x - 0.4).abs() < 1.0e-12);
    assert_eq!(result.delta.z, 0.0);
    assert!(result.collided);
}

#[test]
fn horizontal_sweep_preserves_partial_z_travel_symmetrically() {
    let result = resolve_movement(
        unit_body(),
        Vec3::new(0.0, 0.0, 1.0),
        &[wall_z(1.4)],
        false,
        0.0,
    );

    assert!((result.delta.z - 0.4).abs() < 1.0e-12);
    assert_eq!(result.delta.x, 0.0);
    assert!(result.collided);
}

#[test]
fn horizontal_sweep_slides_along_wall_after_partial_contact() {
    let result = resolve_movement(
        unit_body(),
        Vec3::new(1.0, 0.0, 1.0),
        &[wall_x(1.4)],
        false,
        0.0,
    );

    assert!((result.delta.x - 0.4).abs() < 1.0e-12);
    assert!((result.delta.z - 1.0).abs() < 1.0e-12);
    assert!(result.collided);
}

#[test]
fn horizontal_sweep_stops_at_diagonal_corner_without_discarding_travel() {
    let result = resolve_movement(
        unit_body(),
        Vec3::new(1.0, 0.0, 1.0),
        &[wall_x(1.4), wall_z(1.4)],
        false,
        0.0,
    );

    assert!((result.delta.x - 0.4).abs() < 1.0e-12);
    assert!((result.delta.z - 0.4).abs() < 1.0e-12);
    assert!(result.collided);
}

#[test]
fn vanilla_axis_order_allows_face_flush_sprint_jump_around_corner() {
    let result = resolve_collision_displacement(
        &IsolatedCornerWorld { x: 1, z: 1 },
        Vec3::new(0.7, 64.0, 0.94),
        Aabb {
            half_width: 0.3,
            height: 1.8,
        },
        Vec3::new(0.26, 0.42, -0.36),
        true,
        STEP_HEIGHT,
    )
    .expect("bounded replay resolves");

    assert_eq!(result.delta, Vec3::new(0.26, 0.42, -0.36));
    assert!(!result.collided);
}

#[test]
fn vanilla_axis_order_keeps_straight_tunnelling_blocked() {
    let result = resolve_collision_displacement(
        &SweptWallWorld,
        Vec3::new(0.5, 64.0, 0.5),
        Aabb {
            half_width: 0.3,
            height: 1.8,
        },
        Vec3::new(3.0, 0.0, 0.0),
        true,
        STEP_HEIGHT,
    )
    .expect("bounded tunnelling replay resolves");

    assert!((result.delta.x - 1.2).abs() < 1.0e-12);
    assert!(result.collided);
}

#[test]
fn vanilla_axis_order_steps_onto_half_block_terrace() {
    let result = resolve_collision_displacement(
        &BottomSlabEdgeWorld,
        Vec3::new(0.5, 63.0, 0.5),
        Aabb {
            half_width: 0.3,
            height: 1.8,
        },
        Vec3::new(0.5, 0.0, 0.0),
        true,
        STEP_HEIGHT,
    )
    .expect("bounded terrace replay resolves");

    assert_eq!(result.delta.x, 0.5);
    assert!((result.delta.y - 0.5).abs() < 1.0e-12);
    assert_eq!(result.delta.z, 0.0);
    assert!(result.stepped);
}

#[test]
fn horizontal_sweep_partial_travel_is_symmetric_for_negative_direction() {
    let obstacle = WorldAabb {
        min_x: -1.4,
        min_y: -1.0,
        min_z: -10.0,
        max_x: -0.4,
        max_y: 2.0,
        max_z: 10.0,
    };
    let result = resolve_movement(
        unit_body(),
        Vec3::new(-1.0, 0.0, 0.0),
        &[obstacle],
        false,
        0.0,
    );

    assert!((result.delta.x + 0.4).abs() < 1.0e-12);
    assert!(result.collided);
}

#[test]
fn horizontal_sweep_exact_endpoint_contact_keeps_full_delta() {
    let result = resolve_movement(
        unit_body(),
        Vec3::new(1.0, 0.0, 0.0),
        &[wall_x(2.0)],
        false,
        0.0,
    );

    assert_eq!(result.delta, Vec3::new(1.0, 0.0, 0.0));
    assert!(!result.collided);
}

#[test]
fn public_collision_constructors_reject_invalid_geometry_without_panicking() {
    assert_eq!(BlockCollisionHeight::from_sixteenths(0), None);
    assert_eq!(BlockCollisionHeight::from_sixteenths(17), None);
    assert!(BlockCollisionHeight::from_sixteenths(16).is_some());
    assert_eq!(BlockCollisionBox::from_sixteenths(1, 0, 0, 1, 1, 1), None);
    assert_eq!(BlockCollisionBox::from_fixed_4096([0, 0, 0, 0, 1, 1]), None);
    assert!(BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 24, 16).is_some());
}

#[test]
fn invalid_physics_coefficients_fail_closed() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };
    let invalid = [
        PhysicsConfig {
            gravity: -1.0,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            terminal_velocity: 1.0,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            ground_friction: -0.1,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            air_drag: 1.1,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            vertical_air_drag: -0.1,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            water_drag: 1.1,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            water_buoyancy: -1.0,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            step_height: -0.1,
            ..PhysicsConfig::default()
        },
        PhysicsConfig {
            jump_speed: -0.1,
            ..PhysicsConfig::default()
        },
    ];

    for config in invalid {
        let result = step_entity(
            body,
            &FlatWorld {
                ground_y: 63,
                water_y: None,
            },
            config,
        );
        assert_eq!(result.body.position, body.position);
        assert_eq!(result.body.velocity, Vec3::ZERO);
    }
}

#[test]
fn invalid_sampler_collision_height_fails_closed_without_panicking() {
    struct InvalidSampler;
    impl BlockSampler for InvalidSampler {
        fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
            BlockMaterial::Air
        }

        fn max_collision_box_y(&self) -> u8 {
            0
        }
    }

    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: false,
    };
    let result = step_entity(body, &InvalidSampler, PhysicsConfig::default());
    assert_eq!(result.body.position, body.position);
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(result.horizontal_collision);
}

#[test]
fn cow_fallback_aabb_matches_vanilla_adult_dimensions() {
    assert_eq!(
        Aabb::COW,
        Aabb {
            half_width: 0.45,
            height: 1.4,
        }
    );
}

struct FlatWorld {
    ground_y: i32,
    water_y: Option<i32>,
}

impl BlockSampler for FlatWorld {
    fn material_at(&self, _x: i32, y: i32, _z: i32) -> BlockMaterial {
        if self.water_y == Some(y) {
            BlockMaterial::Water
        } else if y <= self.ground_y {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct OneBlockStepWorld;

impl BlockSampler for OneBlockStepWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 63 || (x == 1 && y == 64 && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct FarmlandToFullBlockStepWorld;

impl BlockSampler for FarmlandToFullBlockStepWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y == 63 && z == 0 && matches!(x, 0 | 1) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.material_at(x, y, z).is_solid().then_some(if x == 0 {
            BlockCollisionHeight::from_sixteenths(15).expect("valid test collision height")
        } else {
            BlockCollisionHeight::FULL_BLOCK
        })
    }
}

struct BottomSlabEdgeWorld;

impl BlockSampler for BottomSlabEdgeWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 62 || (x == 1 && y == 63 && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if y <= 62 {
            emit(BlockCollisionBox::FULL_BLOCK);
        } else if x == 1 && y == 63 && z == 0 {
            emit(
                BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16)
                    .expect("valid test collision box"),
            );
        }
    }
}

struct IsolatedFenceWorld;

impl BlockSampler for IsolatedFenceWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 62 || (x == 1 && y == 63 && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if y <= 62 {
            emit(BlockCollisionBox::FULL_BLOCK);
        } else if x == 1 && y == 63 && z == 0 {
            emit(
                BlockCollisionBox::from_sixteenths(6, 0, 6, 10, 24, 10)
                    .expect("valid test collision box"),
            );
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        24
    }
}

struct StraightBottomStairWorld;

impl BlockSampler for StraightBottomStairWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 62 || (x == 1 && y == 63 && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if y <= 62 {
            emit(BlockCollisionBox::FULL_BLOCK);
        } else if x == 1 && y == 63 && z == 0 {
            emit(
                BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16)
                    .expect("valid test collision box"),
            );
            emit(
                BlockCollisionBox::from_sixteenths(0, 8, 0, 16, 16, 8)
                    .expect("valid test collision box"),
            );
        }
    }
}

struct SlabUnderLowCeilingWorld;

impl BlockSampler for SlabUnderLowCeilingWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 62 || (x == 1 && y == 63 && z == 0) || (x == 1 && y == 64 && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if y <= 62 {
            emit(BlockCollisionBox::FULL_BLOCK);
        } else if x == 1 && y == 63 && z == 0 {
            emit(
                BlockCollisionBox::from_sixteenths(0, 0, 0, 16, 8, 16)
                    .expect("valid test collision box"),
            );
        } else if x == 1 && y == 64 && z == 0 {
            emit(
                BlockCollisionBox::from_sixteenths(0, 12, 0, 16, 16, 16)
                    .expect("valid test collision box"),
            );
        }
    }
}

struct SweptWallWorld;

impl BlockSampler for SweptWallWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if y <= 63 || (x == 2 && matches!(y, 64 | 65) && z == 0) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct IsolatedCornerWorld {
    x: i32,
    z: i32,
}

impl BlockSampler for IsolatedCornerWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if x == self.x && y == 64 && z == self.z {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct JumpCeilingWorld;

impl BlockSampler for JumpCeilingWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if z == 0 && ((x == 0 && y == 63) || (x == 1 && y == 64) || (x == 0 && y == 67)) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct ProjectileWallWorld;

impl BlockSampler for ProjectileWallWorld {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if x == 2 && y == 64 && matches!(z, 0..=2) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

struct PanicSampler;

impl BlockSampler for PanicSampler {
    fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
        panic!("invalid movement must be rejected before sampling")
    }
}

struct CountingAirWorld {
    material_samples: Cell<usize>,
}

impl BlockSampler for CountingAirWorld {
    fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
        self.material_samples
            .set(self.material_samples.get().saturating_add(1));
        BlockMaterial::Air
    }
}

struct FluidRecordingSampler {
    calls: RefCell<Vec<(i32, i32, i32)>>,
    fluid_at: (i32, i32, i32),
}

impl FluidRecordingSampler {
    fn all_air() -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            fluid_at: (i32::MAX, i32::MAX, i32::MAX),
        }
    }

    fn with_fluid_at(x: i32, y: i32, z: i32) -> Self {
        Self {
            calls: RefCell::new(Vec::new()),
            fluid_at: (x, y, z),
        }
    }

    fn sampled(&self) -> Vec<(i32, i32, i32)> {
        self.calls.borrow().clone()
    }
}

impl BlockSampler for FluidRecordingSampler {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.calls.borrow_mut().push((x, y, z));
        if (x, y, z) == self.fluid_at {
            BlockMaterial::Water
        } else {
            BlockMaterial::Air
        }
    }
}

struct DenseCollisionWorld {
    emitted: Cell<usize>,
}

impl BlockSampler for DenseCollisionWorld {
    fn material_at(&self, _x: i32, _y: i32, _z: i32) -> BlockMaterial {
        BlockMaterial::Air
    }

    fn collision_boxes_at(
        &self,
        _x: i32,
        _y: i32,
        _z: i32,
        emit: &mut dyn FnMut(BlockCollisionBox),
    ) {
        for _ in 0..1_024 {
            self.emitted.set(self.emitted.get().saturating_add(1));
            emit(BlockCollisionBox::FULL_BLOCK);
        }
    }
}

fn unit_tick_config() -> PhysicsConfig {
    PhysicsConfig {
        tick_seconds: 1.0,
        gravity: 0.0,
        air_drag: 1.0,
        vertical_air_drag: 1.0,
        step_height: 0.0,
        ..PhysicsConfig::default()
    }
}

#[test]
fn gravity_lands_body_on_solid_ground() {
    let world = FlatWorld {
        ground_y: 63,
        water_y: None,
    };
    let mut body = EntityBody {
        position: Vec3::new(0.5, 66.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: false,
    };

    for _ in 0..40 {
        body = step_entity(body, &world, PhysicsConfig::default()).body;
    }

    assert_eq!(body.position.y, 64.0);
    assert_eq!(body.velocity.y, 0.0);
    assert!(body.on_ground);
}

#[test]
fn air_drag_damps_horizontal_motion() {
    let world = FlatWorld {
        ground_y: -64,
        water_y: None,
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 70.0, 0.5),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: false,
    };

    let stepped = step_entity(body, &world, PhysicsConfig::default()).body;

    assert!(stepped.velocity.x < body.velocity.x);
    assert!(stepped.velocity.y < 0.0);
}

#[test]
fn horizontal_collision_stops_body_at_wall() {
    let world = LocalBlocks {
        solids: vec![
            GridPos::new(0, 63, 0),
            GridPos::new(1, 64, 0),
            GridPos::new(1, 65, 0),
        ],
        fluids: Vec::new(),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &world, PhysicsConfig::default());
    let stepped = result.body;

    assert!(result.horizontal_collision);
    assert!((stepped.position.x - 0.55).abs() < 1.0e-9);
    assert_eq!(stepped.velocity.x, 0.0);
    assert_eq!(stepped.velocity.y, 0.0);
    assert!(stepped.on_ground);
}

#[test]
fn swept_motion_stops_before_a_wall_crossed_in_one_tick() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(80.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &SweptWallWorld, PhysicsConfig::default());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.55).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
}

#[test]
fn diagonal_swept_motion_slides_along_a_wall() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(80.0, 0.0, 10.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &SweptWallWorld, PhysicsConfig::default());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.55).abs() < 1.0e-9);
    assert!(result.body.position.z > body.position.z);
    assert_eq!(result.body.velocity.x, 0.0);
    assert!(result.body.velocity.z > 0.0);
}

#[test]
fn diagonal_sweep_stops_at_an_isolated_positive_corner() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(2.0, 0.0, 2.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
    };

    let result = step_entity(
        body,
        &IsolatedCornerWorld { x: 1, z: 1 },
        unit_tick_config(),
    );

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 0.75).abs() < 1.0e-9);
    assert!((result.body.position.z - 0.75).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
    assert_eq!(result.body.velocity.z, 0.0);
}

#[test]
fn diagonal_sweep_stops_at_an_isolated_negative_corner() {
    let body = EntityBody {
        position: Vec3::new(2.5, 64.0, 2.5),
        velocity: Vec3::new(-2.0, 0.0, -2.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
    };

    let result = step_entity(
        body,
        &IsolatedCornerWorld { x: 1, z: 1 },
        unit_tick_config(),
    );

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 2.25).abs() < 1.0e-9);
    assert!((result.body.position.z - 2.25).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
    assert_eq!(result.body.velocity.z, 0.0);
}

#[test]
fn auto_jump_scans_the_full_vertical_displacement() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(1.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 1.0,
        },
        on_ground: true,
    };
    let config = PhysicsConfig {
        jump_speed: 4.0,
        ..unit_tick_config()
    };

    let result = step_entity(body, &JumpCeilingWorld, config);

    assert!(result.horizontal_collision);
    assert_eq!(result.body.position.y, 66.0);
    assert_eq!(result.body.velocity.y, 0.0);
    assert!(result.body.position.y + result.body.aabb.height <= 67.0);
}

#[test]
fn stop_on_solid_stops_at_the_first_impact_point() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(2.0, 0.0, 1.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
    };
    let config = PhysicsConfig {
        stop_on_solid: true,
        ..unit_tick_config()
    };

    let result = step_entity(body, &ProjectileWallWorld, config);

    assert!(result.horizontal_collision);
    assert_eq!(result.body.position.x, 1.75);
    assert_eq!(result.body.position.z, 1.125);
    assert_eq!(result.body.velocity, Vec3::ZERO);
}

#[test]
fn living_entity_does_not_jump_when_the_raised_path_is_blocked() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(80.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &SweptWallWorld, PhysicsConfig::living_entity());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.55).abs() < 1.0e-9);
    assert_eq!(result.body.position.y, body.position.y);
    assert_eq!(result.body.position.z, body.position.z);
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(result.body.on_ground);
}

#[test]
fn non_finite_displacement_is_rejected_before_world_sampling() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(f64::NAN, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &PanicSampler, PhysicsConfig::default());

    assert_eq!(result.body.position, body.position);
    assert_eq!(result.body.velocity, Vec3::ZERO);
}

#[test]
fn oversized_displacement_is_rejected_before_world_sampling() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(17.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &PanicSampler, unit_tick_config());

    assert_eq!(result.body.position, body.position);
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(result.horizontal_collision);
}

#[test]
fn oversized_scan_volume_is_rejected_before_candidate_collection() {
    let world = CountingAirWorld {
        material_samples: Cell::new(0),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(16.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 10.0,
            height: 16.0,
        },
        on_ground: false,
    };

    let result = step_entity(body, &world, unit_tick_config());

    assert_eq!(result.body.position, body.position);
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(world.material_samples.get() <= 100);
}

#[test]
fn body_overlaps_fluid_samples_narrow_body_once_per_column_layer() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.25,
            height: 1.4,
        },
        on_ground: false,
    };

    let miss = FluidRecordingSampler::all_air();
    assert!(!body_overlaps_fluid(body, &miss));
    assert_eq!(miss.sampled(), vec![(0, 64, 0), (0, 65, 0)]);

    let hit = FluidRecordingSampler::with_fluid_at(0, 64, 0);
    assert!(body_overlaps_fluid(body, &hit));
    assert_eq!(hit.sampled(), vec![(0, 64, 0)]);
}

#[test]
fn body_overlaps_fluid_samples_two_distinct_columns_in_order() {
    let body = EntityBody {
        position: Vec3::new(0.0, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.25,
            height: 1.4,
        },
        on_ground: false,
    };

    let miss = FluidRecordingSampler::all_air();
    assert!(!body_overlaps_fluid(body, &miss));
    assert_eq!(
        miss.sampled(),
        vec![(-1, 64, 0), (-1, 65, 0), (0, 64, 0), (0, 65, 0)]
    );

    let hit = FluidRecordingSampler::with_fluid_at(0, 64, 0);
    assert!(body_overlaps_fluid(body, &hit));
    assert_eq!(hit.sampled(), vec![(-1, 64, 0), (-1, 65, 0), (0, 64, 0)]);
}

#[test]
fn body_overlaps_fluid_samples_four_distinct_columns_without_duplicates() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.55,
            height: 1.4,
        },
        on_ground: false,
    };

    let miss = FluidRecordingSampler::all_air();
    assert!(!body_overlaps_fluid(body, &miss));
    assert_eq!(
        miss.sampled(),
        vec![
            (-1, 64, -1),
            (-1, 65, -1),
            (-1, 64, 1),
            (-1, 65, 1),
            (1, 64, -1),
            (1, 65, -1),
            (1, 64, 1),
            (1, 65, 1),
        ]
    );

    let hit = FluidRecordingSampler::with_fluid_at(1, 64, -1);
    assert!(body_overlaps_fluid(body, &hit));
    assert_eq!(
        hit.sampled(),
        vec![
            (-1, 64, -1),
            (-1, 65, -1),
            (-1, 64, 1),
            (-1, 65, 1),
            (1, 64, -1),
        ]
    );
}

#[test]
fn collision_candidate_collection_stops_at_its_cap() {
    let world = DenseCollisionWorld {
        emitted: Cell::new(0),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(16.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.5,
        },
        on_ground: false,
    };

    let result = step_entity(body, &world, unit_tick_config());

    assert_eq!(result.body.position, body.position);
    assert_eq!(result.body.velocity, Vec3::ZERO);
    assert!(world.emitted.get() <= 8_192 + 1_024);
}

#[test]
fn arrow_projectile_applies_gravity_and_vertical_drag() {
    let world = FlatWorld {
        ground_y: -64,
        water_y: None,
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 70.0, 0.5),
        velocity: Vec3::new(0.0, 1.0, 0.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.25,
        },
        on_ground: false,
    };

    let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

    assert!(stepped.velocity.y < body.velocity.y);
    assert!(stepped.position.y > body.position.y);
}

#[test]
fn arrow_projectile_stops_on_solid_without_step_up() {
    let world = LocalBlocks {
        solids: vec![GridPos::new(1, 64, 0)],
        fluids: Vec::new(),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.25,
            height: 0.25,
        },
        on_ground: true,
    };

    let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

    assert!((stepped.position.x - 0.75).abs() < 1.0e-9);
    let desired_x = body.velocity.x * ARROW_DRAG * TICK_SECONDS;
    let impact_time = (1.0 - (body.position.x + body.aabb.half_width)) / desired_x;
    let desired_y =
        -ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED * TICK_SECONDS * ARROW_DRAG * TICK_SECONDS;
    let expected_y = body.position.y + desired_y * impact_time;
    assert!((stepped.position.y - expected_y).abs() < 1.0e-9);
    assert!((stepped.position.z - body.position.z).abs() < 1.0e-9);
    assert_eq!(stepped.velocity, Vec3::ZERO);
    assert!(stepped.on_ground);
}

#[test]
fn stopped_arrow_projectile_remains_stopped() {
    let world = FlatWorld {
        ground_y: 63,
        water_y: None,
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.25,
            height: 0.25,
        },
        on_ground: true,
    };

    let stepped = step_entity(body, &world, PhysicsConfig::arrow_projectile()).body;

    assert_eq!(stepped, body);
}

struct SupportShortcut<'a> {
    sampler: &'a dyn BlockSampler,
    collision_samples: Cell<usize>,
}

impl BlockSampler for SupportShortcut<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        self.sampler.material_at(x, y, z)
    }

    fn max_collision_box_y(&self) -> u8 {
        self.sampler.max_collision_box_y()
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        self.collision_samples.set(self.collision_samples.get() + 1);
        self.sampler.collision_boxes_at(x, y, z, emit);
    }

    fn stationary_downward_clip(
        &self,
        body: EntityBody,
        displacement_y: f64,
        max_scan_cells: usize,
        max_candidates: usize,
    ) -> Option<f64> {
        stationary_support_clip(
            self,
            body,
            displacement_y,
            max_scan_cells,
            max_candidates,
            4,
        )
    }
}

#[test]
fn support_shortcut_matches_full_scan_for_support_edits_shapes_and_fluids() {
    let flat = FlatWorld {
        ground_y: 63,
        water_y: None,
    };
    let wet = FlatWorld {
        ground_y: 63,
        water_y: Some(64),
    };
    let absent = LocalBlocks {
        solids: Vec::new(),
        fluids: Vec::new(),
    };
    let cases: [(&dyn BlockSampler, Vec3); 7] = [
        (&flat, Vec3::new(-16.0, 64.0, -16.0)),
        (&BottomSlabEdgeWorld, Vec3::new(1.5, 63.5, 0.5)),
        (&BottomSlabEdgeWorld, Vec3::new(1.5, 64.0, 0.5)),
        (&IsolatedFenceWorld, Vec3::new(1.5, 64.5, 0.5)),
        (&FarmlandToFullBlockStepWorld, Vec3::new(0.5, 64.0, 0.5)),
        (&absent, Vec3::new(0.5, 64.0, 0.5)),
        (&wet, Vec3::new(0.5, 64.0, 0.5)),
    ];
    for (sampler, position) in cases {
        let body = EntityBody {
            position,
            velocity: Vec3::ZERO,
            aabb: Aabb::COW,
            on_ground: true,
        };
        let shortcut = SupportShortcut {
            sampler,
            collision_samples: Cell::new(0),
        };
        let mut boxes = SmallVec::new();
        let reference = WorldAabb::for_body(body);
        let dy = -0.08;
        if let Some(proven) = stationary_support_clip(&shortcut, body, dy, 8192, 8192, 4) {
            collision_boxes_for_motion(
                &shortcut,
                reference,
                Vec3::new(0.0, dy, 0.0),
                0.0,
                &mut boxes,
            )
            .unwrap();
            assert_eq!(proven.to_bits(), clip_y(reference, dy, &boxes).to_bits());
        }
        let actual = step_entity(body, &shortcut, PhysicsConfig::living_entity());
        // A transparent wrapper with no hook is the ordinary implementation.
        struct Reference<'a>(&'a dyn BlockSampler);
        impl BlockSampler for Reference<'_> {
            fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
                self.0.material_at(x, y, z)
            }
            fn max_collision_box_y(&self) -> u8 {
                self.0.max_collision_box_y()
            }
            fn collision_boxes_at(
                &self,
                x: i32,
                y: i32,
                z: i32,
                emit: &mut dyn FnMut(BlockCollisionBox),
            ) {
                self.0.collision_boxes_at(x, y, z, emit);
            }
        }
        assert_eq!(
            actual,
            step_entity(body, &Reference(sampler), PhysicsConfig::living_entity())
        );
    }
}

#[test]
fn support_shortcut_samples_one_cell_and_preserves_both_scan_budgets() {
    let flat = FlatWorld {
        ground_y: 63,
        water_y: None,
    };
    let sampler = SupportShortcut {
        sampler: &flat,
        collision_samples: Cell::new(0),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: true,
    };
    assert_eq!(
        stationary_support_clip(&sampler, body, -0.08, 8192, 8192, 1),
        Some(0.0)
    );
    assert_eq!(sampler.collision_samples.get(), 1);
    assert_eq!(
        stationary_support_clip(&sampler, body, -0.08, 0, 8192, 1),
        None
    );
    assert_eq!(
        stationary_support_clip(&sampler, body, -0.08, 8192, 0, 1),
        None
    );
    assert_eq!(
        stationary_support_clip(&sampler, body, -0.08, 8192, 8192, usize::MAX),
        None
    );
    assert_eq!(
        sampler.collision_samples.get(),
        1,
        "budget refusals never sample"
    );
}

#[test]
fn settled_living_body_follows_lowered_support_like_the_general_resolver() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: true,
    };
    let config = PhysicsConfig::living_entity();
    let general = step_entity(
        EntityBody {
            on_ground: false,
            ..body
        },
        &FarmlandToFullBlockStepWorld,
        config,
    );
    let settled = step_entity(body, &FarmlandToFullBlockStepWorld, config);
    assert_eq!(
        general.body.position.y.to_bits(),
        (63.0_f64 + 15.0 / 16.0).to_bits()
    );
    assert!(general.body.on_ground);
    assert_eq!(
        settled.body.position.y.to_bits(),
        general.body.position.y.to_bits()
    );
    assert_eq!(settled, general);
}

#[test]
fn settled_living_body_falls_when_support_is_removed() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: true,
    };
    let world = LocalBlocks {
        solids: Vec::new(),
        fluids: Vec::new(),
    };
    let config = PhysicsConfig::living_entity();
    let general = step_entity(
        EntityBody {
            on_ground: false,
            ..body
        },
        &world,
        config,
    );
    let settled = step_entity(body, &world, config);
    assert!(!settled.body.on_ground);
    assert!(settled.body.position.y < body.position.y);
    assert_eq!(settled, general);
}

#[test]
fn stopped_arrow_embedding_is_not_a_living_support_test() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb {
            half_width: 0.25,
            height: 0.25,
        },
        on_ground: true,
    };
    let world = LocalBlocks {
        solids: Vec::new(),
        fluids: Vec::new(),
    };
    assert_eq!(
        step_entity(body, &world, PhysicsConfig::arrow_projectile()).body,
        body
    );
}

#[test]
fn living_body_starts_full_block_climb_with_jump() {
    let world = LocalBlocks {
        solids: vec![GridPos::new(0, 63, 0), GridPos::new(1, 64, 0)],
        fluids: Vec::new(),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &world, PhysicsConfig::living_entity());
    let stepped = result.body;

    assert!(result.horizontal_collision);
    assert!((stepped.position.x - 0.55).abs() < 1.0e-9);
    assert!(
        stepped.position.y > 64.0 && stepped.position.y < 65.0,
        "{stepped:?}"
    );
    assert!((stepped.velocity.y - LIVING_JUMP_SPEED_BLOCKS_PER_SECOND).abs() < 1.0e-9);
    assert!(!stepped.on_ground);
}

#[test]
fn living_body_steps_from_farmland_to_full_block_without_starting_jump() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0 + 15.0 / 16.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 4.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(
        body,
        &FarmlandToFullBlockStepWorld,
        PhysicsConfig::living_entity(),
    );

    assert!(!result.horizontal_collision);
    assert_eq!(result.body.position.y, 64.0);
    assert_eq!(result.body.velocity.y, 0.0);
    assert!(result.body.on_ground);
    assert!(result.body.position.x > body.position.x);
    let expected_z = body.position.z + body.velocity.z * AIR_DRAG * TICK_SECONDS;
    let expected_velocity_z = body.velocity.z * AIR_DRAG * GROUND_FRICTION;
    assert!((result.body.position.z - expected_z).abs() < 1.0e-9);
    assert!((result.body.velocity.z - expected_velocity_z).abs() < 1.0e-9);
}

#[test]
fn living_body_walks_over_vanilla_bottom_slab_height_difference() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &BottomSlabEdgeWorld, PhysicsConfig::living_entity());

    assert!(!result.horizontal_collision);
    assert_eq!(result.body.position.y, 63.5);
    assert!(result.body.position.x > body.position.x);
    assert_eq!(result.body.velocity.y, 0.0);
    assert!(result.body.on_ground);
}

#[test]
fn diagonal_step_path_over_slab_lands_on_support() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 20.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
    };

    let result = step_entity(body, &BottomSlabEdgeWorld, PhysicsConfig::living_entity());

    assert!(!result.horizontal_collision);
    assert!((result.body.position.x - 1.48).abs() < 1.0e-9);
    assert!((result.body.position.z - 1.48).abs() < 1.0e-9);
    assert!((result.body.position.y - 63.0).abs() < 1.0e-9);
    assert!(result.body.on_ground);
}

#[test]
fn small_body_can_move_beside_an_isolated_fence_post() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.1),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
    };

    let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

    assert!(!result.horizontal_collision);
    assert!(result.body.position.x > body.position.x);
    assert_eq!(result.body.position.y, 63.0);
}

#[test]
fn isolated_fence_post_blocks_its_occupied_center() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
    };

    let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.175).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
}

#[test]
fn overheight_fence_rooted_below_body_still_blocks_motion() {
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: false,
    };

    let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.175).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
}

#[test]
fn swept_motion_cannot_tunnel_through_an_overheight_fence() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.5),
        velocity: Vec3::new(60.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
    };

    let result = step_entity(body, &IsolatedFenceWorld, PhysicsConfig::living_entity());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 1.175).abs() < 1.0e-9);
    assert_eq!(result.body.velocity.x, 0.0);
}

#[test]
fn step_path_is_rejected_when_a_low_ceiling_blocks_the_rise() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.5),
        velocity: Vec3::new(60.0, 0.0, 0.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &SlabUnderLowCeilingWorld, PhysicsConfig::default());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 0.55).abs() < 1.0e-9);
    assert!((result.body.position.y - 63.0).abs() < 1.0e-9);
}

#[test]
fn body_steps_on_lower_tread_of_composed_stair_shape() {
    let body = EntityBody {
        position: Vec3::new(0.5, 63.0, 0.75),
        velocity: Vec3::new(20.0, 0.0, 0.0),
        aabb: Aabb {
            half_width: 0.2,
            height: 0.7,
        },
        on_ground: true,
    };

    let result = step_entity(
        body,
        &StraightBottomStairWorld,
        PhysicsConfig::living_entity(),
    );

    assert!(!result.horizontal_collision);
    assert!(result.body.position.x > body.position.x);
    assert_eq!(result.body.position.y, 63.5);
    assert!(result.body.on_ground);
}

#[test]
fn living_jump_clears_blocked_axis_and_keeps_tangent_motion() {
    let world = LocalBlocks {
        solids: vec![GridPos::new(0, 63, 0), GridPos::new(1, 64, 0)],
        fluids: Vec::new(),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(20.0, 0.0, 4.0),
        aabb: Aabb::COW,
        on_ground: true,
    };

    let result = step_entity(body, &world, PhysicsConfig::living_entity());

    assert!(result.horizontal_collision);
    assert!((result.body.position.x - 0.55).abs() < 1.0e-9);
    assert!(result.body.position.z > body.position.z);
    assert_eq!(result.body.velocity.x, 0.0);
    assert!(result.body.velocity.z > 0.0);
    assert!(result.body.velocity.y > 0.0);
}

#[test]
fn passive_livestock_finish_climbing_one_block() {
    let livestock = [
        (
            "cow",
            Aabb {
                half_width: 0.45,
                height: 1.4,
            },
        ),
        (
            "sheep",
            Aabb {
                half_width: 0.45,
                height: 0.9,
            },
        ),
        (
            "chicken",
            Aabb {
                half_width: 0.2,
                height: 0.7,
            },
        ),
    ];

    for (name, aabb) in livestock {
        let mut body = EntityBody {
            position: Vec3::new(0.5, 64.0, 0.5),
            velocity: Vec3::ZERO,
            aabb,
            on_ground: true,
        };
        let mut reached_step = false;

        for _ in 0..40 {
            body.velocity.x = 2.0;
            body = step_entity(body, &OneBlockStepWorld, PhysicsConfig::living_entity()).body;
            if body.position.x > 1.0 && body.position.y >= 65.0 {
                reached_step = true;
                break;
            }
        }

        assert!(reached_step, "{name} must jump onto a full block");
    }
}

#[test]
fn living_jump_stops_below_solid_ceiling() {
    let world = LocalBlocks {
        solids: vec![GridPos::new(0, 63, 0), GridPos::new(0, 66, 0)],
        fluids: Vec::new(),
    };
    let mut body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(0.0, LIVING_JUMP_SPEED_BLOCKS_PER_SECOND, 0.0),
        aabb: Aabb::COW,
        on_ground: false,
    };

    for _ in 0..4 {
        body = step_entity(body, &world, PhysicsConfig::living_entity()).body;
    }

    assert!(body.position.y + body.aabb.height <= 66.0);
    assert!(body.velocity.y <= 0.0);
}

#[test]
fn water_applies_buoyancy_and_stronger_drag() {
    let world = FlatWorld {
        ground_y: 60,
        water_y: Some(64),
    };
    let body = EntityBody {
        position: Vec3::new(0.5, 64.0, 0.5),
        velocity: Vec3::new(1.0, -1.0, 0.0),
        aabb: Aabb::COW,
        on_ground: false,
    };

    let result = step_entity(body, &world, PhysicsConfig::default());

    assert!(result.in_fluid);
    assert!(result.body.velocity.x < 1.0);
    assert!(result.body.velocity.y > -1.0);
}

#[test]
fn material_ids_classify_fluids_apart_from_solids() {
    let ids = BlockMaterialIds::new(0, Some(5), Some(6));

    assert_eq!(ids.classify(0), BlockMaterial::Air);
    assert_eq!(ids.classify(5), BlockMaterial::Water);
    assert_eq!(ids.classify(6), BlockMaterial::Lava);
    assert_eq!(ids.classify(7), BlockMaterial::Solid);
}

#[test]
fn material_ids_classify_passable_apart_from_fluids() {
    let ids = BlockMaterialIds::new(0, Some(5), Some(6)).with_passable(vec![6, 7]);

    assert_eq!(ids.classify(5), BlockMaterial::Water);
    assert_eq!(ids.classify(6), BlockMaterial::Lava);
    assert_eq!(ids.classify(7), BlockMaterial::Air);
}

#[derive(Debug)]
struct LocalBlocks {
    solids: Vec<GridPos>,
    fluids: Vec<GridPos>,
}

impl BlockSampler for LocalBlocks {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        let pos = GridPos::new(x, y, z);
        if self.fluids.contains(&pos) {
            BlockMaterial::Water
        } else if self.solids.contains(&pos) {
            BlockMaterial::Solid
        } else {
            BlockMaterial::Air
        }
    }
}

#[test]
fn long_fall_clamps_velocity_before_ground_impact() {
    let world = LocalBlocks {
        solids: Vec::new(),
        fluids: Vec::new(),
    };
    let mut body = EntityBody {
        position: Vec3::new(0.5, 120.0, 0.5),
        velocity: Vec3::ZERO,
        aabb: Aabb::COW,
        on_ground: false,
    };

    for _ in 0..200 {
        body = step_entity(body, &world, PhysicsConfig::default()).body;
    }

    assert!(body.velocity.y >= TERMINAL_VELOCITY_BLOCKS_PER_SECOND);
    assert!(body.velocity.y < -10.0);
    assert!(!body.on_ground);
}

#[test]
fn representative_movement_boundary_matrix_uses_step_entity_authority() {
    #[derive(Clone, Copy)]
    enum Boundary {
        StepUp,
        LongFall,
        NonFinite,
    }

    for (name, boundary) in [
        ("farmland step-up", Boundary::StepUp),
        ("terminal long fall", Boundary::LongFall),
        ("non-finite rejection", Boundary::NonFinite),
    ] {
        match boundary {
            Boundary::StepUp => {
                let body = EntityBody {
                    position: Vec3::new(0.5, 63.0 + 15.0 / 16.0, 0.5),
                    velocity: Vec3::new(20.0, 0.0, 4.0),
                    aabb: Aabb::COW,
                    on_ground: true,
                };
                let result = step_entity(
                    body,
                    &FarmlandToFullBlockStepWorld,
                    PhysicsConfig::living_entity(),
                );
                assert!(!result.horizontal_collision, "{name}");
                assert_eq!(result.body.position.y, 64.0, "{name}");
                assert_eq!(result.body.velocity.y, 0.0, "{name}");
                assert!(result.body.on_ground, "{name}");
            }
            Boundary::LongFall => {
                let world = LocalBlocks {
                    solids: Vec::new(),
                    fluids: Vec::new(),
                };
                let mut body = EntityBody {
                    position: Vec3::new(0.5, 120.0, 0.5),
                    velocity: Vec3::ZERO,
                    aabb: Aabb::COW,
                    on_ground: false,
                };
                for _ in 0..200 {
                    body = step_entity(body, &world, PhysicsConfig::default()).body;
                }
                assert_eq!(
                    body.velocity.y, TERMINAL_VELOCITY_BLOCKS_PER_SECOND,
                    "{name} must reach the exact configured clamp"
                );
                let clamped = step_entity(body, &world, PhysicsConfig::default()).body;
                assert_eq!(
                    clamped.velocity.y, TERMINAL_VELOCITY_BLOCKS_PER_SECOND,
                    "{name} must remain at the clamp on the next step"
                );
                assert!(!clamped.on_ground, "{name}");
            }
            Boundary::NonFinite => {
                let body = EntityBody {
                    position: Vec3::new(0.5, 64.0, 0.5),
                    velocity: Vec3::new(f64::NAN, 0.0, 0.0),
                    aabb: Aabb::COW,
                    on_ground: true,
                };
                let result = step_entity(body, &PanicSampler, PhysicsConfig::default());
                assert_eq!(result.body.position, body.position, "{name}");
                assert_eq!(result.body.velocity, Vec3::ZERO, "{name}");
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct GridPos {
    x: i32,
    y: i32,
    z: i32,
}

impl GridPos {
    #[must_use]
    const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }
}
