//! # mc-physics
//!
//! Block physics, collisions, fluids.
//!
//! Part of the Solaris engine.

use mth::{ceil_i32, floor_i32};
use smallvec::SmallVec;

pub mod entity_collision_26_1_2;
pub mod mth;

pub const TICK_SECONDS: f64 = 0.05;
pub const GRAVITY_BLOCKS_PER_SECOND_SQUARED: f64 = 32.0;
pub const TERMINAL_VELOCITY_BLOCKS_PER_SECOND: f64 = -78.4;
pub const GROUND_FRICTION: f64 = 0.6;
pub const AIR_DRAG: f64 = 0.98;
pub const WATER_DRAG: f64 = 0.8;
/// Downward drift applied to swimming terrestrial bodies while immersed.
/// Vanilla `LivingEntity` water gravity is `getEffectiveGravity() / 16` per
/// tick (`0.08 / 16 = 0.005` blocks/tick), i.e. `0.005 * 20 / 0.05 = 2.0`
/// blocks/s^2. Vanilla living entities therefore sink slowly unless a
/// `FloatGoal` jump drives them up; there is no passive surface buoyancy.
pub const WATER_SINK_BLOCKS_PER_SECOND_SQUARED: f64 = 2.0;
/// Continuous swim drive toward a shallow immersion threshold. Vanilla uses
/// depth-gated FloatGoal jump impulses; this kernel uses a bounded steady drive
/// rather than item buoyancy. With drag, submerged rise settles at +0.2 blocks/s.
pub const TERRESTRIAL_SWIM_LIFT_BLOCKS_PER_SECOND_SQUARED: f64 = 3.0;
pub const WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED: f64 = 7.0;
pub const STEP_HEIGHT: f64 = 0.6;
pub const LIVING_JUMP_SPEED_BLOCKS_PER_SECOND: f64 = 0.419_999_986_886_978_15 / TICK_SECONDS;
pub const ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED: f64 = 9.8;
pub const ARROW_DRAG: f64 = 0.99;
pub const ARROW_WATER_DRAG: f64 = 0.6;

const MAX_DISPLACEMENT_PER_STEP: f64 = 16.0;
const MAX_BODY_EXTENT: f64 = 16.0;
const MAX_COLLISION_SCAN_CELLS: usize = 8_192;
const MAX_COLLISION_CANDIDATES: usize = 8_192;
const COLLISION_CANDIDATES_INLINE: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub const ZERO: Self = Self::new(0.0, 0.0, 0.0);

    #[must_use]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

/// Returns whether a displacement is finite and fits inside the supplied
/// Euclidean per-step budget. Invalid budgets fail closed.
#[must_use]
pub fn displacement_within_limit(displacement: Vec3, max_distance: f64) -> bool {
    if !max_distance.is_finite() || max_distance < 0.0 {
        return false;
    }
    let distance_sq = displacement.x.mul_add(
        displacement.x,
        displacement
            .y
            .mul_add(displacement.y, displacement.z * displacement.z),
    );
    distance_sq.is_finite() && distance_sq <= max_distance * max_distance
}

/// Returns whether an authoritative pose contains only finite values and stays
/// inside the supplied absolute coordinate limits. Invalid limits fail closed.
#[must_use]
pub fn authoritative_pose_within_limits(
    position: Vec3,
    yaw: f32,
    pitch: f32,
    horizontal_limit: f64,
    vertical_limit: f64,
) -> bool {
    horizontal_limit.is_finite()
        && horizontal_limit >= 0.0
        && vertical_limit.is_finite()
        && vertical_limit >= 0.0
        && position.x.is_finite()
        && position.y.is_finite()
        && position.z.is_finite()
        && yaw.is_finite()
        && pitch.is_finite()
        && position.x.abs() <= horizontal_limit
        && position.z.abs() <= horizontal_limit
        && position.y.abs() <= vertical_limit
}

/// Returns the number of linear sweep samples required so no axis advances
/// farther than `max_step` between samples. Invalid inputs fail closed.
#[must_use]
pub fn sweep_sample_count(displacement: Vec3, max_step: f64) -> Option<usize> {
    if !max_step.is_finite() || max_step <= 0.0 {
        return None;
    }
    if !displacement.x.is_finite() || !displacement.y.is_finite() || !displacement.z.is_finite() {
        return None;
    }
    let max_axis = displacement
        .x
        .abs()
        .max(displacement.y.abs())
        .max(displacement.z.abs());
    let steps = (max_axis / max_step).ceil().max(1.0);
    (steps <= usize::MAX as f64).then_some(steps as usize)
}

#[must_use]
pub fn horizontal_movement_residual_within_limit(
    requested: Vec3,
    resolved: Vec3,
    max_squared_residual: f64,
) -> bool {
    if !max_squared_residual.is_finite() || max_squared_residual < 0.0 {
        return false;
    }
    if !requested.x.is_finite()
        || !requested.z.is_finite()
        || !resolved.x.is_finite()
        || !resolved.z.is_finite()
    {
        return false;
    }
    let residual_x = requested.x - resolved.x;
    let residual_z = requested.z - resolved.z;
    residual_x.mul_add(residual_x, residual_z * residual_z) <= max_squared_residual
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CollisionResolvedDisplacement {
    pub delta: Vec3,
    pub grounded: bool,
    pub stepped: bool,
    pub collided: bool,
}

/// Resolves one already-requested entity displacement against block collision
/// shapes without applying gravity, drag, acceleration, or jump impulses.
///
/// This is the collision-only primitive used when an external authority (for
/// example the vanilla client) has already integrated movement and the server
/// only needs to verify that the reported displacement is physically compatible
/// with sliding/stepping collision resolution.
#[must_use]
pub fn resolve_collision_displacement<S: BlockSampler>(
    sampler: &S,
    position: Vec3,
    aabb: Aabb,
    desired: Vec3,
    was_on_ground: bool,
    step_height: f64,
) -> Option<CollisionResolvedDisplacement> {
    let finite = [
        position.x,
        position.y,
        position.z,
        aabb.half_width,
        aabb.height,
        desired.x,
        desired.y,
        desired.z,
        step_height,
    ]
    .into_iter()
    .all(f64::is_finite);
    if !finite
        || aabb.half_width <= 0.0
        || aabb.half_width > MAX_BODY_EXTENT
        || aabb.height <= 0.0
        || aabb.height > MAX_BODY_EXTENT
        || !(0.0..=MAX_BODY_EXTENT).contains(&step_height)
        || !bounded_displacement(desired)
    {
        return None;
    }

    let body = EntityBody {
        position,
        velocity: Vec3::ZERO,
        aabb,
        on_ground: was_on_ground,
    };
    let world_box = WorldAabb::for_body(body);
    let mut collision_boxes = SmallVec::new();
    collision_boxes_for_motion(
        sampler,
        world_box,
        desired,
        step_height,
        &mut collision_boxes,
    )?;
    let resolved = resolve_vanilla_collision_movement(
        world_box,
        desired,
        &collision_boxes,
        was_on_ground,
        step_height,
    );
    Some(CollisionResolvedDisplacement {
        delta: resolved.delta,
        grounded: resolved.grounded,
        stepped: resolved.stepped,
        collided: resolved.collided,
    })
}

#[must_use]
pub fn rotation_is_finite(yaw: f32, pitch: f32) -> bool {
    yaw.is_finite() && pitch.is_finite()
}

/// Clamps a finite world position to the supplied absolute horizontal/vertical
/// limits. Invalid positions or limits fail closed.
#[must_use]
pub fn clamp_world_position(
    position: Vec3,
    horizontal_limit: f64,
    vertical_limit: f64,
) -> Option<Vec3> {
    if !horizontal_limit.is_finite()
        || horizontal_limit < 0.0
        || !vertical_limit.is_finite()
        || vertical_limit < 0.0
        || !position.x.is_finite()
        || !position.y.is_finite()
        || !position.z.is_finite()
    {
        return None;
    }
    Some(Vec3::new(
        position.x.clamp(-horizontal_limit, horizontal_limit),
        position.y.clamp(-vertical_limit, vertical_limit),
        position.z.clamp(-horizontal_limit, horizontal_limit),
    ))
}

/// Tests strict overlap between a body AABB and an obstacle AABB after shrinking
/// the obstacle by `deflation` on every face. Invalid boxes fail closed.
#[must_use]
pub fn aabb_intersects_deflated_obstacle(
    body: [f64; 6],
    obstacle: [f64; 6],
    deflation: f64,
) -> bool {
    if !deflation.is_finite()
        || deflation < 0.0
        || body
            .iter()
            .chain(obstacle.iter())
            .any(|value| !value.is_finite())
        || body[0] > body[3]
        || body[1] > body[4]
        || body[2] > body[5]
        || obstacle[0] > obstacle[3]
        || obstacle[1] > obstacle[4]
        || obstacle[2] > obstacle[5]
    {
        return false;
    }
    body[0] < obstacle[3] - deflation
        && body[3] > obstacle[0] + deflation
        && body[1] < obstacle[4] - deflation
        && body[4] > obstacle[1] + deflation
        && body[2] < obstacle[5] - deflation
        && body[5] > obstacle[2] + deflation
}

/// Computes the retained fall-start height after an accepted movement update.
/// Non-finite heights fail closed by returning `None`.
#[must_use]
pub fn next_fall_start_y(
    old_y: f64,
    old_fall_start_y: f64,
    old_on_ground: bool,
    old_in_water: bool,
    new_y: f64,
    new_on_ground: bool,
    new_in_water: bool,
) -> Option<f64> {
    if !old_y.is_finite() || !old_fall_start_y.is_finite() || !new_y.is_finite() {
        return None;
    }
    Some(if new_on_ground || new_in_water {
        new_y
    } else if old_on_ground || old_in_water {
        old_y.max(new_y)
    } else {
        old_fall_start_y.max(new_y)
    })
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub half_width: f64,
    pub height: f64,
}

impl Aabb {
    pub const COW: Self = Self {
        half_width: 0.45,
        height: 1.4,
    };
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMaterial {
    Air,
    Solid,
    Water,
    Lava,
}

impl BlockMaterial {
    #[must_use]
    pub const fn is_solid(self) -> bool {
        matches!(self, Self::Solid)
    }

    #[must_use]
    pub const fn is_fluid(self) -> bool {
        matches!(self, Self::Water | Self::Lava)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockCollisionHeight(u8);

impl BlockCollisionHeight {
    pub const FULL_BLOCK: Self = Self(16);

    #[must_use]
    pub const fn from_sixteenths(height: u8) -> Option<Self> {
        if height == 0 || height > 16 {
            return None;
        }
        Some(Self(height))
    }

    #[must_use]
    pub const fn as_blocks(self) -> f64 {
        self.0 as f64 / 16.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BlockCollisionBox {
    coordinates: [i16; 6],
}

impl BlockCollisionBox {
    pub const FULL_BLOCK: Self = Self {
        coordinates: [0, 0, 0, 4096, 4096, 4096],
    };

    #[must_use]
    pub const fn from_sixteenths(
        min_x: u8,
        min_y: u8,
        min_z: u8,
        max_x: u8,
        max_y: u8,
        max_z: u8,
    ) -> Option<Self> {
        if min_x >= max_x || min_y >= max_y || min_z >= max_z {
            return None;
        }
        Some(Self {
            coordinates: [
                min_x as i16 * 256,
                min_y as i16 * 256,
                min_z as i16 * 256,
                max_x as i16 * 256,
                max_y as i16 * 256,
                max_z as i16 * 256,
            ],
        })
    }

    #[must_use]
    pub const fn from_fixed_4096(coordinates: [i16; 6]) -> Option<Self> {
        if coordinates[0] >= coordinates[3]
            || coordinates[1] >= coordinates[4]
            || coordinates[2] >= coordinates[5]
        {
            return None;
        }
        Some(Self { coordinates })
    }

    #[must_use]
    pub const fn coordinates(self) -> [i16; 6] {
        self.coordinates
    }

    pub fn as_blocks(self) -> [f64; 6] {
        self.coordinates.map(|value| f64::from(value) / 4096.0)
    }
}

/// Upper bound for the dense per-state lookup table, in state ids. Real
/// vanilla registries stay far below this; anything at or above it keeps the
/// exact linear-scan semantics without allocating the table.
const DENSE_CLASS_TABLE_LIMIT: u32 = 1 << 20;

/// Precomputed [`BlockMaterialIds`] classification and collision height for
/// one numeric block state id. Unlisted states are solid full blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct DenseBlockClass {
    material: BlockMaterial,
    collision_height: Option<BlockCollisionHeight>,
}

impl Default for DenseBlockClass {
    fn default() -> Self {
        Self {
            material: BlockMaterial::Solid,
            collision_height: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockMaterialIds {
    pub air: u32,
    pub water: Vec<u32>,
    pub lava: Vec<u32>,
    pub passable: Vec<u32>,
    collision_heights: Vec<(u32, BlockCollisionHeight)>,
    dense_classes: Box<[DenseBlockClass]>,
}

impl BlockMaterialIds {
    #[must_use]
    pub fn new(air: u32, water: Option<u32>, lava: Option<u32>) -> Self {
        let water = match water {
            Some(state) => Vec::from([state]),
            None => Vec::new(),
        };
        let lava = match lava {
            Some(state) => Vec::from([state]),
            None => Vec::new(),
        };
        let mut ids = Self {
            air,
            water,
            lava,
            passable: Vec::new(),
            collision_heights: Vec::new(),
            dense_classes: Box::new([]),
        };
        ids.rebuild_dense_classes();
        ids
    }

    #[must_use]
    pub fn with_passable(mut self, passable: Vec<u32>) -> Self {
        self.passable = passable;
        self.rebuild_dense_classes();
        self
    }

    #[must_use]
    pub fn with_water_states(mut self, water: Vec<u32>) -> Self {
        if !water.is_empty() {
            self.water = water;
            self.rebuild_dense_classes();
        }
        self
    }

    #[must_use]
    pub fn with_lava_states(mut self, lava: Vec<u32>) -> Self {
        if !lava.is_empty() {
            self.lava = lava;
            self.rebuild_dense_classes();
        }
        self
    }

    #[must_use]
    pub fn with_collision_height(mut self, states: Vec<u32>, height: BlockCollisionHeight) -> Self {
        self.collision_heights
            .extend(states.into_iter().map(|state| (state, height)));
        self.rebuild_dense_classes();
        self
    }

    fn rebuild_dense_classes(&mut self) {
        let max_state = self
            .water
            .iter()
            .chain(&self.lava)
            .chain(&self.passable)
            .copied()
            .chain(self.collision_heights.iter().map(|&(state, _)| state))
            .max()
            .unwrap_or(self.air)
            .max(self.air);
        self.dense_classes = if max_state >= DENSE_CLASS_TABLE_LIMIT {
            Box::new([])
        } else {
            let mut dense = vec![DenseBlockClass::default(); max_state as usize + 1];
            dense[self.air as usize].material = BlockMaterial::Air;
            for (states, material) in [
                (&self.water, BlockMaterial::Water),
                (&self.lava, BlockMaterial::Lava),
                (&self.passable, BlockMaterial::Air),
            ] {
                for &state in states {
                    let class = &mut dense[state as usize];
                    if class.material == BlockMaterial::Solid {
                        class.material = material;
                    }
                }
            }
            for &(state, height) in &self.collision_heights {
                let class = &mut dense[state as usize];
                if class.collision_height.is_none() {
                    class.collision_height = Some(height);
                }
            }
            dense.into_boxed_slice()
        };
    }

    fn classify_by_scan(&self, state_id: u32) -> BlockMaterial {
        if state_id == self.air {
            BlockMaterial::Air
        } else if self.water.contains(&state_id) {
            BlockMaterial::Water
        } else if self.lava.contains(&state_id) {
            BlockMaterial::Lava
        } else if self.passable.contains(&state_id) {
            BlockMaterial::Air
        } else {
            BlockMaterial::Solid
        }
    }

    #[must_use]
    pub fn classify(&self, state_id: u32) -> BlockMaterial {
        self.dense_classes
            .get(state_id as usize)
            .map_or_else(|| self.classify_by_scan(state_id), |class| class.material)
    }

    #[must_use]
    pub fn collision_height(&self, state_id: u32) -> Option<BlockCollisionHeight> {
        let class = self
            .dense_classes
            .get(state_id as usize)
            .copied()
            .unwrap_or_else(|| DenseBlockClass {
                material: self.classify_by_scan(state_id),
                collision_height: self
                    .collision_heights
                    .iter()
                    .find_map(|&(state, height)| (state == state_id).then_some(height)),
            });
        if class.material != BlockMaterial::Solid {
            return None;
        }
        Some(
            class
                .collision_height
                .unwrap_or(BlockCollisionHeight::FULL_BLOCK),
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EntityBody {
    pub position: Vec3,
    pub velocity: Vec3,
    pub aabb: Aabb,
    pub on_ground: bool,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhysicsConfig {
    pub tick_seconds: f64,
    pub gravity: f64,
    pub terminal_velocity: f64,
    pub ground_friction: f64,
    pub air_drag: f64,
    pub vertical_air_drag: f64,
    pub water_drag: f64,
    pub water_buoyancy: f64,
    /// Fish travel moves before drag and adds its idle sink after drag.
    pub water_drag_after_move: bool,
    pub water_sink: f64,
    pub step_height: f64,
    pub jump_speed: f64,
    pub stop_on_solid: bool,
}

impl Default for PhysicsConfig {
    fn default() -> Self {
        Self {
            tick_seconds: TICK_SECONDS,
            gravity: GRAVITY_BLOCKS_PER_SECOND_SQUARED,
            terminal_velocity: TERMINAL_VELOCITY_BLOCKS_PER_SECOND,
            ground_friction: GROUND_FRICTION,
            air_drag: AIR_DRAG,
            vertical_air_drag: 1.0,
            water_drag: WATER_DRAG,
            water_buoyancy: WATER_BUOYANCY_BLOCKS_PER_SECOND_SQUARED,
            water_drag_after_move: false,
            water_sink: 0.0,
            step_height: STEP_HEIGHT,
            jump_speed: 0.0,
            stop_on_solid: false,
        }
    }
}

impl PhysicsConfig {
    /// Terrestrial living bodies (sheep, pigs, cows, …) swim with a bounded
    /// immersion model, not floating-item buoyancy: vanilla
    /// `LivingEntity.travelInWater` only drags and sinks, while upward drive
    /// comes from `FloatGoal` jumps gated on fluid depth. The kernel uses a
    /// shallow swim threshold above the feet (see `step_entity`), so no
    /// per-type checks are needed here.
    #[must_use]
    pub fn living_entity() -> Self {
        Self {
            jump_speed: LIVING_JUMP_SPEED_BLOCKS_PER_SECOND,
            water_buoyancy: TERRESTRIAL_SWIM_LIFT_BLOCKS_PER_SECOND_SQUARED,
            ..Self::default()
        }
    }

    /// Vanilla fish propel themselves through water instead of receiving the
    /// generic floating-item buoyancy used by the shared body kernel.
    #[must_use]
    pub fn aquatic_entity() -> Self {
        Self {
            water_drag: 0.9,
            water_buoyancy: 0.0,
            ..Self::living_entity()
        }
    }

    /// AbstractFish.travelInWater: move(delta), scale(.9), add(0,-.005,0).
    #[must_use]
    pub fn fish_entity() -> Self {
        Self {
            water_drag_after_move: true,
            water_sink: 0.005 * 20.0,
            jump_speed: 0.0,
            step_height: 0.0,
            ..Self::aquatic_entity()
        }
    }

    /// Squid.travel only moves; its pulsed damping is applied by Squid.aiStep,
    /// not again by LivingEntity's generic water travel.
    #[must_use]
    pub fn squid_entity() -> Self {
        Self {
            water_drag: 1.0,
            jump_speed: 0.0,
            step_height: 0.0,
            gravity: 0.08 * 20.0 * 20.0,
            air_drag: 0.0,
            vertical_air_drag: 0.980_000_019_073_486_3,
            ..Self::aquatic_entity()
        }
    }

    #[must_use]
    pub fn arrow_projectile() -> Self {
        Self {
            gravity: ARROW_GRAVITY_BLOCKS_PER_SECOND_SQUARED,
            air_drag: ARROW_DRAG,
            vertical_air_drag: ARROW_DRAG,
            water_drag: ARROW_WATER_DRAG,
            water_buoyancy: 0.0,
            step_height: 0.0,
            stop_on_solid: true,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StepResult {
    pub body: EntityBody,
    pub in_fluid: bool,
    pub horizontal_collision: bool,
}

pub trait BlockSampler {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial;

    /// Largest local `max_y` emitted by [`BlockSampler::collision_boxes_at`],
    /// in sixteenths of a block. Implementations that emit over-height boxes
    /// must override this so collision scans include boxes rooted below a body.
    fn max_collision_box_y(&self) -> u8 {
        16
    }

    fn collision_height_at(&self, x: i32, y: i32, z: i32) -> Option<BlockCollisionHeight> {
        self.material_at(x, y, z)
            .is_solid()
            .then_some(BlockCollisionHeight::FULL_BLOCK)
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if let Some(height) = self.collision_height_at(x, y, z)
            && let Some(collision_box) =
                BlockCollisionBox::from_sixteenths(0, 0, 0, 16, height.0, 16)
        {
            emit(collision_box);
        }
    }

    /// Optional exact clipping of a stationary downward move. `Some` must prove
    /// the ordinary collision scan succeeds within both limits and return its
    /// bit-identical clip. `None` retains that scan. Fluids are checked first.
    fn stationary_downward_clip(
        &self,
        _body: EntityBody,
        _displacement_y: f64,
        _max_scan_cells: usize,
        _max_candidates: usize,
    ) -> Option<f64> {
        None
    }
}

pub fn step_entity<S: BlockSampler>(
    mut body: EntityBody,
    sampler: &S,
    config: PhysicsConfig,
) -> StepResult {
    let was_on_ground = body.on_ground;
    let raw_displacement = Vec3::new(
        body.velocity.x * config.tick_seconds,
        body.velocity.y * config.tick_seconds,
        body.velocity.z * config.tick_seconds,
    );
    let jump_displacement = config.jump_speed * config.tick_seconds;
    if !valid_step_input(body, config)
        || !bounded_displacement(raw_displacement)
        || !jump_displacement.is_finite()
        || jump_displacement.abs() > MAX_DISPLACEMENT_PER_STEP
    {
        return rejected_step(body, false, raw_displacement);
    }

    let in_fluid = body_overlaps_fluid(body, sampler);
    if config.stop_on_solid && body.on_ground && body.velocity == Vec3::ZERO {
        return StepResult {
            body,
            in_fluid,
            horizontal_collision: false,
        };
    }
    let mut collision_boxes = SmallVec::new();
    if !in_fluid && body.on_ground && body.velocity == Vec3::ZERO {
        let settled_velocity_y = (-config.gravity * config.tick_seconds * config.vertical_air_drag)
            .max(config.terminal_velocity);
        let settled_displacement_y = settled_velocity_y * config.tick_seconds;
        if settled_displacement_y < 0.0
            && bounded_displacement(Vec3::new(0.0, settled_displacement_y, 0.0))
        {
            let start_box = WorldAabb::for_body(body);
            let clipped_y = sampler
                .stationary_downward_clip(
                    body,
                    settled_displacement_y,
                    MAX_COLLISION_SCAN_CELLS,
                    MAX_COLLISION_CANDIDATES,
                )
                .or_else(|| {
                    collision_boxes_for_motion(
                        sampler,
                        start_box,
                        Vec3::new(0.0, settled_displacement_y, 0.0),
                        0.0,
                        &mut collision_boxes,
                    )
                    .map(|()| clip_y(start_box, settled_displacement_y, &collision_boxes))
                });
            if clipped_y.is_some_and(|dy| dy.to_bits() == 0.0_f64.to_bits()) {
                // Shortened support can still lower the body. Only exact zero
                // displacement preserves the general resolver's body.
                return StepResult {
                    body,
                    in_fluid,
                    horizontal_collision: false,
                };
            }
        }
    }
    if in_fluid {
        // Living bodies sink gently like vanilla (`gravity / 16` effective
        // water gravity). The bounded swim drive only assists a body already
        // swimming upward (a FloatGoal-style impulse): passive immersion keeps
        // sinking instead of pinning just below the surface.
        if config.jump_speed > 0.0 && config.water_buoyancy > 0.0 {
            body.velocity.y -= WATER_SINK_BLOCKS_PER_SECOND_SQUARED * config.tick_seconds;
            if body.velocity.y > 0.0 && fluid_reaches_swim_threshold(body, sampler) {
                body.velocity.y += config.water_buoyancy * config.tick_seconds;
            }
        } else {
            body.velocity.y += config.water_buoyancy * config.tick_seconds;
        }
        if !config.water_drag_after_move {
            body.velocity.x *= config.water_drag;
            body.velocity.y *= config.water_drag;
            body.velocity.z *= config.water_drag;
        }
    } else {
        body.velocity.y -= config.gravity * config.tick_seconds;
        body.velocity.x *= config.air_drag;
        body.velocity.y *= config.vertical_air_drag;
        body.velocity.z *= config.air_drag;
    }
    body.velocity.y = body.velocity.y.max(config.terminal_velocity);

    let desired = Vec3::new(
        body.velocity.x * config.tick_seconds,
        body.velocity.y * config.tick_seconds,
        body.velocity.z * config.tick_seconds,
    );
    let start_box = WorldAabb::for_body(body);
    if !bounded_displacement(desired) {
        return rejected_step(body, in_fluid, desired);
    }
    let Some(()) = collision_boxes_for_motion(
        sampler,
        start_box,
        desired,
        config.step_height.max(0.0),
        &mut collision_boxes,
    ) else {
        return rejected_step(body, in_fluid, desired);
    };
    let movement = if config.stop_on_solid {
        resolve_until_first_impact(start_box, desired, &collision_boxes)
    } else {
        resolve_movement(
            start_box,
            desired,
            &collision_boxes,
            was_on_ground,
            config.step_height,
        )
    };
    body.position.x += movement.delta.x;
    body.position.y += movement.delta.y;
    body.position.z += movement.delta.z;

    let clipped_x = axis_was_clipped(desired.x, movement.delta.x);
    let clipped_z = axis_was_clipped(desired.z, movement.delta.z);
    let horizontal_collision = clipped_x || clipped_z;
    if clipped_x {
        body.velocity.x = 0.0;
    }
    if clipped_z {
        body.velocity.z = 0.0;
    }
    if axis_was_clipped(desired.y, movement.delta.y) {
        body.velocity.y = 0.0;
    }

    if movement.collided && config.stop_on_solid {
        body.velocity = Vec3::ZERO;
        body.on_ground = true;
        return StepResult {
            body,
            in_fluid,
            horizontal_collision,
        };
    }

    body.on_ground = movement.grounded;
    let can_start_jump =
        horizontal_collision && !movement.stepped && (was_on_ground || movement.grounded);
    if can_start_jump {
        body.on_ground = true;
        if try_start_jump(
            &mut body,
            desired,
            &collision_boxes,
            false,
            config.jump_speed,
        ) {
            let jump_box = WorldAabb::for_body(body);
            let jump_dy = body.velocity.y * config.tick_seconds;
            let mut jump_collision_boxes = SmallVec::new();
            let jump_scan = collision_boxes_for_motion(
                sampler,
                jump_box,
                Vec3::new(0.0, jump_dy, 0.0),
                0.0,
                &mut jump_collision_boxes,
            );
            let clipped_jump_dy =
                jump_scan.map_or(0.0, |()| clip_y(jump_box, jump_dy, &jump_collision_boxes));
            body.position.y += clipped_jump_dy;
            if jump_scan.is_none() || axis_was_clipped(jump_dy, clipped_jump_dy) {
                body.velocity.y = 0.0;
            }
            body.on_ground = false;
        } else {
            body.on_ground = movement.grounded;
        }
    }
    // Vanilla jumpOutOfFluid uses a 0.3-block/tick impulse only when the elevated
    // destination is free of both collision and liquid, not just a clear ceiling.
    if in_fluid
        && horizontal_collision
        && !movement.stepped
        && !was_on_ground
        && !movement.grounded
        && config.jump_speed > 0.0
        && config.water_buoyancy > 0.0
        && {
            let rise =
                0.600_000_023_841_857_9 + body.velocity.y * config.tick_seconds - movement.delta.y;
            let mut exit = body;
            exit.position.y += rise;
            clip_y(WorldAabb::for_body(body), rise, &collision_boxes) >= rise
                && !body_overlaps_fluid(exit, sampler)
        }
    {
        body.velocity.y = 0.300_000_011_920_928_96 / config.tick_seconds;
        body.on_ground = false;
    }
    if body.on_ground {
        body.velocity.y = 0.0;
        body.velocity.x *= config.ground_friction;
        body.velocity.z *= config.ground_friction;
    }
    if in_fluid && config.water_drag_after_move {
        body.velocity.x *= config.water_drag;
        body.velocity.y *= config.water_drag;
        body.velocity.z *= config.water_drag;
        body.velocity.y -= config.water_sink;
    }

    StepResult {
        body,
        in_fluid,
        horizontal_collision,
    }
}

fn valid_step_input(body: EntityBody, config: PhysicsConfig) -> bool {
    let finite_values = [
        body.position.x,
        body.position.y,
        body.position.z,
        body.velocity.x,
        body.velocity.y,
        body.velocity.z,
        body.aabb.half_width,
        body.aabb.height,
        config.tick_seconds,
        config.gravity,
        config.terminal_velocity,
        config.ground_friction,
        config.air_drag,
        config.vertical_air_drag,
        config.water_drag,
        config.water_buoyancy,
        config.water_sink,
        config.step_height,
        config.jump_speed,
    ];
    finite_values.into_iter().all(f64::is_finite)
        && config.tick_seconds > 0.0
        && config.gravity >= 0.0
        && config.terminal_velocity <= 0.0
        && (0.0..=1.0).contains(&config.ground_friction)
        && (0.0..=1.0).contains(&config.air_drag)
        && (0.0..=1.0).contains(&config.vertical_air_drag)
        && (0.0..=1.0).contains(&config.water_drag)
        && config.water_buoyancy >= 0.0
        && config.water_sink >= 0.0
        && config.step_height >= 0.0
        && config.jump_speed >= 0.0
        && body.aabb.half_width > 0.0
        && body.aabb.half_width <= MAX_BODY_EXTENT
        && body.aabb.height > 0.0
        && body.aabb.height <= MAX_BODY_EXTENT
}

fn bounded_displacement(displacement: Vec3) -> bool {
    [displacement.x, displacement.y, displacement.z]
        .into_iter()
        .all(|axis| axis.is_finite() && axis.abs() <= MAX_DISPLACEMENT_PER_STEP)
}

fn rejected_step(mut body: EntityBody, in_fluid: bool, displacement: Vec3) -> StepResult {
    body.velocity = Vec3::ZERO;
    StepResult {
        body,
        in_fluid,
        horizontal_collision: !displacement.x.is_finite()
            || !displacement.z.is_finite()
            || displacement.x != 0.0
            || displacement.z != 0.0,
    }
}

#[derive(Debug, Clone, Copy)]
struct WorldAabb {
    min_x: f64,
    min_y: f64,
    min_z: f64,
    max_x: f64,
    max_y: f64,
    max_z: f64,
}

impl WorldAabb {
    fn for_body(body: EntityBody) -> Self {
        Self {
            min_x: body.position.x - body.aabb.half_width,
            min_y: body.position.y,
            min_z: body.position.z - body.aabb.half_width,
            max_x: body.position.x + body.aabb.half_width,
            max_y: body.position.y + body.aabb.height,
            max_z: body.position.z + body.aabb.half_width,
        }
    }

    fn moved(self, delta: Vec3) -> Self {
        Self {
            min_x: self.min_x + delta.x,
            min_y: self.min_y + delta.y,
            min_z: self.min_z + delta.z,
            max_x: self.max_x + delta.x,
            max_y: self.max_y + delta.y,
            max_z: self.max_z + delta.z,
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedMovement {
    delta: Vec3,
    grounded: bool,
    stepped: bool,
    collided: bool,
}

#[derive(Debug, Clone, Copy)]
struct SweepHit {
    time: f64,
    blocked_x: bool,
    blocked_y: bool,
    blocked_z: bool,
}

/// Gather collision candidates from the cells an expanded body actually enters.
///
/// The old broad-phase visited the complete rectangular bounding volume.  A
/// diagonal move through that volume has a much smaller swept surface: start
/// with the body footprint, then add only each leading cell slab as its face
/// crosses a grid boundary.  The axis order is fixed for deterministic
/// candidate emission when faces cross together.
fn collision_boxes_for_motion<S: BlockSampler>(
    sampler: &S,
    body: WorldAabb,
    desired: Vec3,
    step_height: f64,
    boxes: &mut SmallVec<[WorldAabb; COLLISION_CANDIDATES_INLINE]>,
) -> Option<()> {
    // Fill in place: returning this inline buffer copies over 1.5 KiB per scan.
    boxes.clear();
    let max_local_y = f64::from(sampler.max_collision_box_y()) / 16.0;
    if max_local_y <= 0.0 {
        return None;
    }

    let mut ranges = [
        CellRange::new(floor_i32(body.min_x), ceil_i32(body.max_x) - 1),
        CellRange::new(
            floor_i32(body.min_y - step_height - max_local_y) + 1,
            ceil_i32(body.max_y + step_height) - 1,
        ),
        CellRange::new(floor_i32(body.min_z), ceil_i32(body.max_z) - 1),
    ];
    let target = [
        CellRange::new(
            floor_i32(body.min_x + desired.x.min(0.0)),
            ceil_i32(body.max_x + desired.x.max(0.0)) - 1,
        ),
        CellRange::new(
            floor_i32(body.min_y + desired.y.min(-step_height) - max_local_y) + 1,
            ceil_i32(body.max_y + desired.y.max(step_height)) - 1,
        ),
        CellRange::new(
            floor_i32(body.min_z + desired.z.min(0.0)),
            ceil_i32(body.max_z + desired.z.max(0.0)) - 1,
        ),
    ];
    let lower = [
        body.min_x,
        body.min_y - step_height - max_local_y,
        body.min_z,
    ];
    let upper = [body.max_x, body.max_y + step_height, body.max_z];
    let displacement = [desired.x, desired.y, desired.z];
    // Reject an oversized swept volume before sampling anything: the
    // per-cell walk below only enforces the cap once a cell is visited.
    let swept = inclusive_range_len(target[0].min, target[0].max)
        .checked_mul(inclusive_range_len(target[1].min, target[1].max))?
        .checked_mul(inclusive_range_len(target[2].min, target[2].max))?;
    if swept > MAX_COLLISION_SCAN_CELLS {
        return None;
    }
    let mut scanned = 0;
    scan_collision_range(sampler, ranges, boxes, &mut scanned)?;

    let mut next: [f64; 3] = std::array::from_fn(|axis| {
        next_axis_crossing(
            displacement[axis],
            lower[axis],
            upper[axis],
            ranges[axis],
            target[axis],
        )
    });
    loop {
        let time = next.into_iter().fold(f64::INFINITY, f64::min);
        if time > 1.0 {
            break;
        }
        for axis in 0..3 {
            if next[axis] != time {
                continue;
            }
            if displacement[axis] > 0.0 {
                ranges[axis].max += 1;
            } else {
                ranges[axis].min -= 1;
            }
            let leading = if displacement[axis] > 0.0 {
                ranges[axis].max
            } else {
                ranges[axis].min
            };
            scan_collision_slab(sampler, ranges, axis, leading, boxes, &mut scanned)?;
            next[axis] += displacement[axis].abs().recip();
            if ranges[axis] == target[axis] {
                next[axis] = f64::INFINITY;
            }
        }
    }
    (ranges == target).then_some(())
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CellRange {
    min: i32,
    max: i32,
}

impl CellRange {
    const fn new(min: i32, max: i32) -> Self {
        Self { min, max }
    }
}

fn next_axis_crossing(
    displacement: f64,
    lower: f64,
    upper: f64,
    current: CellRange,
    target: CellRange,
) -> f64 {
    if displacement > 0.0 && current.max < target.max {
        (upper.ceil() - upper) / displacement
    } else if displacement < 0.0 && current.min > target.min {
        (lower - lower.floor()) / -displacement
    } else {
        f64::INFINITY
    }
}

fn scan_collision_range<S: BlockSampler>(
    sampler: &S,
    ranges: [CellRange; 3],
    boxes: &mut SmallVec<[WorldAabb; COLLISION_CANDIDATES_INLINE]>,
    scanned: &mut usize,
) -> Option<()> {
    for x in ranges[0].min..=ranges[0].max {
        for y in ranges[1].min..=ranges[1].max {
            for z in ranges[2].min..=ranges[2].max {
                scan_collision_cell(sampler, x, y, z, boxes, scanned)?;
            }
        }
    }
    Some(())
}

fn scan_collision_slab<S: BlockSampler>(
    sampler: &S,
    ranges: [CellRange; 3],
    axis: usize,
    leading: i32,
    boxes: &mut SmallVec<[WorldAabb; COLLISION_CANDIDATES_INLINE]>,
    scanned: &mut usize,
) -> Option<()> {
    match axis {
        0 => {
            for y in ranges[1].min..=ranges[1].max {
                for z in ranges[2].min..=ranges[2].max {
                    scan_collision_cell(sampler, leading, y, z, boxes, scanned)?;
                }
            }
        }
        1 => {
            for x in ranges[0].min..=ranges[0].max {
                for z in ranges[2].min..=ranges[2].max {
                    scan_collision_cell(sampler, x, leading, z, boxes, scanned)?;
                }
            }
        }
        2 => {
            for x in ranges[0].min..=ranges[0].max {
                for y in ranges[1].min..=ranges[1].max {
                    scan_collision_cell(sampler, x, y, leading, boxes, scanned)?;
                }
            }
        }
        _ => unreachable!("three collision axes"),
    }
    Some(())
}

fn scan_collision_cell<S: BlockSampler>(
    sampler: &S,
    x: i32,
    y: i32,
    z: i32,
    boxes: &mut SmallVec<[WorldAabb; COLLISION_CANDIDATES_INLINE]>,
    scanned: &mut usize,
) -> Option<()> {
    *scanned = scanned.checked_add(1)?;
    if *scanned > MAX_COLLISION_SCAN_CELLS {
        return None;
    }
    let mut overflowed = false;
    sampler.collision_boxes_at(x, y, z, &mut |collision_box| {
        if boxes.len() >= MAX_COLLISION_CANDIDATES {
            overflowed = true;
            return;
        }
        let [
            box_min_x,
            box_min_y,
            box_min_z,
            box_max_x,
            box_max_y,
            box_max_z,
        ] = collision_box.as_blocks();
        boxes.push(WorldAabb {
            min_x: f64::from(x) + box_min_x,
            min_y: f64::from(y) + box_min_y,
            min_z: f64::from(z) + box_min_z,
            max_x: f64::from(x) + box_max_x,
            max_y: f64::from(y) + box_max_y,
            max_z: f64::from(z) + box_max_z,
        });
    });
    (!overflowed).then_some(())
}

#[cfg(test)]
fn collision_boxes_for_motion_volume_scan<S: BlockSampler>(
    sampler: &S,
    body: WorldAabb,
    desired: Vec3,
    step_height: f64,
    boxes: &mut SmallVec<[WorldAabb; COLLISION_CANDIDATES_INLINE]>,
) -> Option<()> {
    boxes.clear();
    let max_local_y = f64::from(sampler.max_collision_box_y()) / 16.0;
    if max_local_y <= 0.0 {
        return None;
    }
    let ranges = [
        CellRange::new(
            floor_i32(body.min_x + desired.x.min(0.0)),
            ceil_i32(body.max_x + desired.x.max(0.0)) - 1,
        ),
        CellRange::new(
            floor_i32(body.min_y + desired.y.min(-step_height) - max_local_y) + 1,
            ceil_i32(body.max_y + desired.y.max(step_height)) - 1,
        ),
        CellRange::new(
            floor_i32(body.min_z + desired.z.min(0.0)),
            ceil_i32(body.max_z + desired.z.max(0.0)) - 1,
        ),
    ];
    let cells = inclusive_range_len(ranges[0].min, ranges[0].max)
        .checked_mul(inclusive_range_len(ranges[1].min, ranges[1].max))?
        .checked_mul(inclusive_range_len(ranges[2].min, ranges[2].max))?;
    if cells > MAX_COLLISION_SCAN_CELLS {
        return None;
    }
    let mut scanned = 0;
    scan_collision_range(sampler, ranges, boxes, &mut scanned)
}

fn inclusive_range_len(min: i32, max: i32) -> usize {
    if min > max {
        return 0;
    }
    (i64::from(max) - i64::from(min) + 1) as usize
}

/// Prove a stationary downward clip is exactly zero from an actual supporting
/// box. The caller supplies a bound covering every cell's emitted box count.
/// If either ordinary scan budget cannot be proved, retain the full scan.
pub fn stationary_support_clip<S: BlockSampler>(
    sampler: &S,
    body: EntityBody,
    displacement_y: f64,
    max_scan_cells: usize,
    max_candidates: usize,
    max_boxes_per_cell: usize,
) -> Option<f64> {
    if displacement_y >= 0.0 || !displacement_y.is_finite() || max_boxes_per_cell == 0 {
        return None;
    }
    let body = WorldAabb::for_body(body);
    let min_x = floor_i32(body.min_x);
    let max_x = ceil_i32(body.max_x).checked_sub(1)?;
    let min_z = floor_i32(body.min_z);
    let max_z = ceil_i32(body.max_z).checked_sub(1)?;
    let local_height = f64::from(sampler.max_collision_box_y()) / 16.0;
    if local_height <= 0.0 {
        return None;
    }
    let min_y = floor_i32(body.min_y + displacement_y - local_height).checked_add(1)?;
    let max_y = ceil_i32(body.max_y).checked_sub(1)?;
    let cells = inclusive_range_len(min_x, max_x)
        .checked_mul(inclusive_range_len(min_y, max_y))?
        .checked_mul(inclusive_range_len(min_z, max_z))?;
    if cells > max_scan_cells || cells.checked_mul(max_boxes_per_cell)? > max_candidates {
        return None;
    }
    // Full blocks and slabs directly beneath the feet cover the common case.
    // Over-height support rooted lower simply retains the general resolver.
    let support_y = ceil_i32(body.min_y).checked_sub(1)?;
    if !(min_y..=max_y).contains(&support_y) {
        return None;
    }
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let mut supported = false;
            sampler.collision_boxes_at(x, support_y, z, &mut |collision_box| {
                let [x0, _, z0, x1, y1, z1] = collision_box.as_blocks();
                supported |= f64::from(support_y) + y1 == body.min_y
                    && body.max_x > f64::from(x) + x0
                    && body.min_x < f64::from(x) + x1
                    && body.max_z > f64::from(z) + z0
                    && body.min_z < f64::from(z) + z1;
            });
            if supported {
                return Some(0.0);
            }
        }
    }
    None
}

type HorizontalResolver = fn(WorldAabb, Vec3, &[WorldAabb]) -> Vec3;

fn resolve_movement(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    was_on_ground: bool,
    step_height: f64,
) -> ResolvedMovement {
    resolve_movement_with(
        body,
        desired,
        collision_boxes,
        was_on_ground,
        step_height,
        resolve_horizontal_sweep,
    )
}

fn resolve_vanilla_collision_movement(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    was_on_ground: bool,
    step_height: f64,
) -> ResolvedMovement {
    resolve_movement_with(
        body,
        desired,
        collision_boxes,
        was_on_ground,
        step_height,
        resolve_horizontal_axes,
    )
}

fn resolve_movement_with(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    was_on_ground: bool,
    step_height: f64,
    resolve_horizontal: HorizontalResolver,
) -> ResolvedMovement {
    let base = resolve_axes(body, desired, collision_boxes, resolve_horizontal);
    let base_grounded = desired.y < 0.0 && axis_was_clipped(desired.y, base.y);
    let horizontal_clipped =
        axis_was_clipped(desired.x, base.x) || axis_was_clipped(desired.z, base.z);
    if step_height <= 0.0 || !horizontal_clipped || !(was_on_ground || base_grounded) {
        return ResolvedMovement {
            delta: base,
            grounded: base_grounded,
            stepped: false,
            collided: axis_was_clipped(desired.x, base.x)
                || axis_was_clipped(desired.y, base.y)
                || axis_was_clipped(desired.z, base.z),
        };
    }

    let raised = clip_y(body, step_height, collision_boxes);
    let raised_box = body.moved(Vec3::new(0.0, raised, 0.0));
    let stepped_horizontal = resolve_horizontal(
        raised_box,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    let after_horizontal = raised_box.moved(stepped_horizontal);
    let requested_down = desired.y - raised;
    let stepped_down = clip_y(after_horizontal, requested_down, collision_boxes);
    let stepped = Vec3::new(
        stepped_horizontal.x,
        raised + stepped_down,
        stepped_horizontal.z,
    );

    if horizontal_distance_squared(stepped) <= horizontal_distance_squared(base) {
        return ResolvedMovement {
            delta: base,
            grounded: base_grounded,
            stepped: false,
            collided: true,
        };
    }

    ResolvedMovement {
        delta: stepped,
        grounded: requested_down < 0.0 && axis_was_clipped(requested_down, stepped_down),
        stepped: true,
        collided: axis_was_clipped(desired.x, stepped.x)
            || axis_was_clipped(desired.y, stepped.y)
            || axis_was_clipped(desired.z, stepped.z),
    }
}

fn resolve_axes(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    resolve_horizontal: HorizontalResolver,
) -> Vec3 {
    let dy = clip_y(body, desired.y, collision_boxes);
    let after_y = body.moved(Vec3::new(0.0, dy, 0.0));
    let horizontal = resolve_horizontal(
        after_y,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    Vec3::new(horizontal.x, dy, horizontal.z)
}

fn resolve_until_first_impact(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> ResolvedMovement {
    let Some(hit) = first_sweep_hit(body, desired, collision_boxes) else {
        return ResolvedMovement {
            delta: desired,
            grounded: false,
            stepped: false,
            collided: false,
        };
    };

    ResolvedMovement {
        delta: Vec3::new(
            desired.x * hit.time,
            desired.y * hit.time,
            desired.z * hit.time,
        ),
        grounded: desired.y < 0.0 && hit.blocked_y,
        stepped: false,
        collided: true,
    }
}

fn resolve_horizontal_axes(body: WorldAabb, desired: Vec3, collision_boxes: &[WorldAabb]) -> Vec3 {
    // Vanilla 26.1.2 resolves the larger horizontal component first. This
    // permits the first axis to clear a face-flush corner before the second
    // axis is clipped, while each axis still scans the complete displacement.
    if desired.x.abs() < desired.z.abs() {
        let dz = clip_z(body, desired.z, collision_boxes);
        let dx = clip_x(
            body.moved(Vec3::new(0.0, 0.0, dz)),
            desired.x,
            collision_boxes,
        );
        Vec3::new(dx, 0.0, dz)
    } else {
        let dx = clip_x(body, desired.x, collision_boxes);
        let dz = clip_z(
            body.moved(Vec3::new(dx, 0.0, 0.0)),
            desired.z,
            collision_boxes,
        );
        Vec3::new(dx, 0.0, dz)
    }
}

fn clip_x(body: WorldAabb, mut dx: f64, collision_boxes: &[WorldAabb]) -> f64 {
    for obstacle in collision_boxes {
        if body.min_y < obstacle.max_y
            && body.max_y > obstacle.min_y
            && body.min_z < obstacle.max_z
            && body.max_z > obstacle.min_z
        {
            if dx > 0.0 && body.max_x <= obstacle.min_x {
                dx = dx.min(obstacle.min_x - body.max_x);
            } else if dx < 0.0 && body.min_x >= obstacle.max_x {
                dx = dx.max(obstacle.max_x - body.min_x);
            }
        }
    }
    dx
}

fn clip_z(body: WorldAabb, mut dz: f64, collision_boxes: &[WorldAabb]) -> f64 {
    for obstacle in collision_boxes {
        if body.min_x < obstacle.max_x
            && body.max_x > obstacle.min_x
            && body.min_y < obstacle.max_y
            && body.max_y > obstacle.min_y
        {
            if dz > 0.0 && body.max_z <= obstacle.min_z {
                dz = dz.min(obstacle.min_z - body.max_z);
            } else if dz < 0.0 && body.min_z >= obstacle.max_z {
                dz = dz.max(obstacle.max_z - body.min_z);
            }
        }
    }
    dz
}

fn resolve_horizontal_sweep(
    mut body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> Vec3 {
    let mut resolved = Vec3::ZERO;
    let mut remaining = desired;

    // At most two horizontal normals can be removed. A third pass only applies
    // the remaining tangent movement after the second contact.
    for _ in 0..3 {
        let Some(hit) = first_sweep_hit(body, remaining, collision_boxes) else {
            resolved.x += remaining.x;
            resolved.z += remaining.z;
            break;
        };

        let travelled = Vec3::new(remaining.x * hit.time, 0.0, remaining.z * hit.time);
        resolved.x += travelled.x;
        resolved.z += travelled.z;
        body = body.moved(travelled);

        let remainder = 1.0 - hit.time;
        remaining.x *= remainder;
        remaining.z *= remainder;
        if hit.blocked_x {
            remaining.x = 0.0;
        }
        if hit.blocked_z {
            remaining.z = 0.0;
        }
        if remaining.x == 0.0 && remaining.z == 0.0 {
            break;
        }
    }

    Vec3::new(resolved.x, 0.0, resolved.z)
}

fn first_sweep_hit(
    body: WorldAabb,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
) -> Option<SweepHit> {
    const TIME_EPSILON: f64 = 1.0e-12;

    let mut nearest: Option<SweepHit> = None;
    for obstacle in collision_boxes {
        let Some((entry_x, exit_x)) = sweep_axis_interval(
            body.min_x,
            body.max_x,
            obstacle.min_x,
            obstacle.max_x,
            desired.x,
        ) else {
            continue;
        };
        let Some((entry_y, exit_y)) = sweep_axis_interval(
            body.min_y,
            body.max_y,
            obstacle.min_y,
            obstacle.max_y,
            desired.y,
        ) else {
            continue;
        };
        let Some((entry_z, exit_z)) = sweep_axis_interval(
            body.min_z,
            body.max_z,
            obstacle.min_z,
            obstacle.max_z,
            desired.z,
        ) else {
            continue;
        };

        let entry = entry_x.max(entry_y).max(entry_z);
        let exit = exit_x.min(exit_y).min(exit_z);
        if entry > exit + TIME_EPSILON || exit < 0.0 || !(-TIME_EPSILON..=1.0).contains(&entry) {
            continue;
        }

        let time = entry.max(0.0);
        let hit = SweepHit {
            time,
            blocked_x: desired.x != 0.0 && (entry_x - entry).abs() <= TIME_EPSILON,
            blocked_y: desired.y != 0.0 && (entry_y - entry).abs() <= TIME_EPSILON,
            blocked_z: desired.z != 0.0 && (entry_z - entry).abs() <= TIME_EPSILON,
        };
        if !hit.blocked_x && !hit.blocked_y && !hit.blocked_z {
            continue;
        }

        match &mut nearest {
            Some(current) if (current.time - time).abs() <= TIME_EPSILON => {
                current.blocked_x |= hit.blocked_x;
                current.blocked_y |= hit.blocked_y;
                current.blocked_z |= hit.blocked_z;
            }
            Some(current) if current.time < time => {}
            _ => nearest = Some(hit),
        }
    }
    nearest
}

fn sweep_axis_interval(
    body_min: f64,
    body_max: f64,
    obstacle_min: f64,
    obstacle_max: f64,
    movement: f64,
) -> Option<(f64, f64)> {
    if movement > 0.0 {
        Some((
            (obstacle_min - body_max) / movement,
            (obstacle_max - body_min) / movement,
        ))
    } else if movement < 0.0 {
        Some((
            (obstacle_max - body_min) / movement,
            (obstacle_min - body_max) / movement,
        ))
    } else if body_min < obstacle_max && body_max > obstacle_min {
        Some((f64::NEG_INFINITY, f64::INFINITY))
    } else {
        None
    }
}

fn clip_y(body: WorldAabb, mut dy: f64, collision_boxes: &[WorldAabb]) -> f64 {
    for obstacle in collision_boxes {
        if body.min_x < obstacle.max_x
            && body.max_x > obstacle.min_x
            && body.min_z < obstacle.max_z
            && body.max_z > obstacle.min_z
        {
            if dy > 0.0 && body.max_y <= obstacle.min_y {
                dy = dy.min(obstacle.min_y - body.max_y);
            } else if dy < 0.0 && body.min_y >= obstacle.max_y {
                dy = dy.max(obstacle.max_y - body.min_y);
            }
        }
    }
    dy
}

fn axis_was_clipped(requested: f64, resolved: f64) -> bool {
    (requested - resolved).abs() > 1.0e-12
}

fn horizontal_distance_squared(delta: Vec3) -> f64 {
    delta.x * delta.x + delta.z * delta.z
}

fn body_overlaps_fluid<S: BlockSampler>(body: EntityBody, sampler: &S) -> bool {
    let min_y = floor_i32(body.position.y);
    let max_y = floor_i32(body.position.y + body.aabb.height);
    let half = body.aabb.half_width;
    let x_lo = floor_i32(body.position.x - half);
    let x_hi = floor_i32(body.position.x + half);
    let z_lo = floor_i32(body.position.z - half);
    let z_hi = floor_i32(body.position.z + half);
    let xs = [x_lo, x_hi];
    let zs = [z_lo, z_hi];
    let xs = if x_lo == x_hi { &xs[..1] } else { &xs[..] };
    let zs = if z_lo == z_hi { &zs[..1] } else { &zs[..] };
    xs.iter().any(|&x| {
        zs.iter()
            .any(|&z| (min_y..=max_y).any(|y| sampler.material_at(x, y, z).is_fluid()))
    })
}

/// Common terrestrial mobs use vanilla's 0.4-block fluid jump threshold.
/// Keep the probe below the midpoint of smaller bodies so they can breathe.
fn fluid_reaches_swim_threshold<S: BlockSampler>(body: EntityBody, sampler: &S) -> bool {
    let probe_y = floor_i32(body.position.y + 0.4_f64.min(body.aabb.height * 0.5));
    sampler
        .material_at(
            floor_i32(body.position.x),
            probe_y,
            floor_i32(body.position.z),
        )
        .is_fluid()
}

fn try_start_jump(
    body: &mut EntityBody,
    desired: Vec3,
    collision_boxes: &[WorldAabb],
    stepped_up: bool,
    jump_speed: f64,
) -> bool {
    if !body.on_ground || stepped_up || jump_speed <= 0.0 {
        return false;
    }
    let body_box = WorldAabb::for_body(*body);
    let raised = clip_y(body_box, 1.0, collision_boxes);
    if axis_was_clipped(1.0, raised) {
        return false;
    }
    let raised_box = body_box.moved(Vec3::new(0.0, raised, 0.0));
    let raised_horizontal = resolve_horizontal_sweep(
        raised_box,
        Vec3::new(desired.x, 0.0, desired.z),
        collision_boxes,
    );
    if axis_was_clipped(desired.x, raised_horizontal.x)
        || axis_was_clipped(desired.z, raised_horizontal.z)
    {
        return false;
    }
    body.velocity.y = body.velocity.y.max(jump_speed);
    true
}

#[cfg(test)]
#[path = "physics_tests.rs"]
mod tests;

#[cfg(test)]
mod aquatic_physics_tests;
