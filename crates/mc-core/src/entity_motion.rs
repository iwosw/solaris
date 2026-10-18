//! Native bounded navigation and look/move/jump controls for blueprint bodies.
//! No session state or per-tick allocation: collision uses the shared physics
//! kernel; swimming probes require known water across the whole swept body.

use crate::blueprint::MovementClass;
use crate::species::AquaticControl;
use mc_physics::mth::{self, floor_i32};
use mc_physics::{Aabb, BlockMaterial, BlockSampler, EntityBody, PhysicsConfig, Vec3};

const EPSILON: f64 = 1.0e-7;
const WANDER_EPISODE: u64 = 120;
const WANDER_WALK: u64 = 40;

pub(crate) struct MotionInput {
    pub body: EntityBody,
    pub movement: MovementClass,
    /// Standing locomotion decision, separate from combat momentum.
    pub drive: [f64; 3],
    pub impulse: [f64; 2],
    pub rotation: [f32; 3],
    pub look_target: Option<[f64; 3]>,
    pub climbs_walls: bool,
    pub aquatic_control: AquaticControl,
    pub air_supply: i32,
    pub levitation: f32,
    pub slow_falling: bool,
    pub id: i32,
    pub tick: u64,
}

pub(crate) struct MotionResult {
    pub body: EntityBody,
    pub impulse: [f64; 2],
    pub rotation: [f32; 3],
    pub air_supply: i32,
    pub drowning: bool,
}

fn unit(id: i32, epoch: u64) -> f64 {
    let mut hash = (id as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(epoch.wrapping_mul(0xBF58_476D_1CE4_E5B9));
    hash ^= hash >> 29;
    hash = hash.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    hash ^= hash >> 32;
    hash as f64 / u64::MAX as f64
}

pub(crate) fn wander(movement: MovementClass, id: i32, tick: u64, speed: f32) -> [f64; 3] {
    if matches!(
        movement,
        MovementClass::Stationary
            | MovementClass::Item
            | MovementClass::Player
            | MovementClass::Projectile
    ) {
        return [0.0; 3];
    }
    let phase = tick.wrapping_add((id as u64).wrapping_mul(0x9E37_79B9));
    if phase % WANDER_EPISODE >= WANDER_WALK {
        return [0.0; 3];
    }
    let epoch = phase / WANDER_EPISODE;
    let angle = unit(id, epoch) * std::f64::consts::TAU;
    let speed = f64::from(speed) * 0.25;
    [
        f64::from(mth::cos(angle)) * speed,
        if matches!(
            movement,
            MovementClass::Flyer | MovementClass::Swimmer | MovementClass::Amphibious
        ) {
            (unit(id, epoch.wrapping_add(0x41)) * 2.0 - 1.0) * speed * 0.5
        } else {
            0.0
        },
        f64::from(mth::sin(angle)) * speed,
    ]
}

pub(crate) fn steer(
    movement: MovementClass,
    position: [f64; 3],
    target: [f64; 3],
    speed: f64,
) -> [f64; 3] {
    if movement == MovementClass::Stationary {
        return [0.0; 3];
    }
    let dy = if matches!(
        movement,
        MovementClass::Flyer | MovementClass::Swimmer | MovementClass::Amphibious
    ) {
        target[1] - position[1]
    } else {
        0.0
    };
    let delta = [target[0] - position[0], dy, target[2] - position[2]];
    let length = (delta[0] * delta[0] + dy * dy + delta[2] * delta[2]).sqrt();
    let scale =
        (speed * mc_physics::TICK_SECONDS / length.max(0.001)).min(1.0) / mc_physics::TICK_SECONDS;
    delta.map(|value| value * scale)
}

/// Panic has no idle part of a wander episode. A known attacker supplies the
/// escape direction; otherwise use the entity's deterministic episode heading.
pub(crate) fn panic(
    movement: MovementClass,
    id: i32,
    tick: u64,
    speed: f64,
    position: [f64; 3],
    source: Option<[f64; 3]>,
) -> [f64; 3] {
    if movement == MovementClass::Stationary {
        return [0.0; 3];
    }
    if let Some(source) = source {
        let target = [
            position[0] + (position[0] - source[0]) * 16.0,
            position[1] + (position[1] - source[1]) * 16.0,
            position[2] + (position[2] - source[2]) * 16.0,
        ];
        let drive = steer(movement, position, target, speed);
        if drive.iter().any(|value| value.abs() > EPSILON) {
            return drive;
        }
    }
    let epoch = tick / WANDER_EPISODE;
    let angle = unit(id, epoch) * std::f64::consts::TAU;
    [
        f64::from(mth::cos(angle)) * speed,
        if matches!(
            movement,
            MovementClass::Flyer | MovementClass::Swimmer | MovementClass::Amphibious
        ) {
            (unit(id, epoch.wrapping_add(0x41)) * 2.0 - 1.0) * speed * 0.5
        } else {
            0.0
        },
        f64::from(mth::sin(angle)) * speed,
    ]
}

/// AbstractFish's dry-air boundary; amphibious bodies do not suffocate on land.
pub(crate) fn breathe(air: i32, water: bool, movement: MovementClass) -> (i32, bool) {
    if movement != MovementClass::Swimmer || water {
        return (300, false);
    }
    let next = air.saturating_sub(1);
    if next <= -20 {
        (0, true)
    } else {
        (next, false)
    }
}

fn water_body<S: BlockSampler>(sampler: &S, position: Vec3, aabb: Aabb) -> bool {
    if !position.x.is_finite()
        || !position.y.is_finite()
        || !position.z.is_finite()
        || !aabb.half_width.is_finite()
        || !aabb.height.is_finite()
        || aabb.half_width <= 0.0
        || aabb.height <= 0.0
    {
        return false;
    }
    let min = [
        floor_i32(position.x - aabb.half_width + EPSILON),
        floor_i32(position.y + EPSILON),
        floor_i32(position.z - aabb.half_width + EPSILON),
    ];
    let max = [
        floor_i32(position.x + aabb.half_width - EPSILON),
        floor_i32(position.y + aabb.height - EPSILON),
        floor_i32(position.z + aabb.half_width - EPSILON),
    ];
    let cells = (i64::from(max[0]) - i64::from(min[0]) + 1)
        .saturating_mul(i64::from(max[1]) - i64::from(min[1]) + 1)
        .saturating_mul(i64::from(max[2]) - i64::from(min[2]) + 1);
    if !(1..=4096).contains(&cells) {
        return false;
    }
    for x in min[0]..=max[0] {
        for y in min[1]..=max[1] {
            for z in min[2]..=max[2] {
                if sampler.material_at(x, y, z) != BlockMaterial::Water {
                    return false;
                }
            }
        }
    }
    true
}

fn water_path<S: BlockSampler>(sampler: &S, body: EntityBody, velocity: Vec3) -> bool {
    let dt = mc_physics::TICK_SECONDS;
    let span = velocity.x.abs().max(velocity.y.abs()).max(velocity.z.abs()) * dt;
    if !span.is_finite() || span > 2.0 {
        return false;
    }
    let steps = (span / 0.2).ceil().max(1.0) as u32;
    (1..=steps).all(|step| {
        let t = dt * f64::from(step) / f64::from(steps);
        water_body(
            sampler,
            Vec3::new(
                body.position.x + velocity.x * t,
                body.position.y + velocity.y * t,
                body.position.z + velocity.z * t,
            ),
            body.aabb,
        )
    })
}

/// Deterministic local alternatives, not an unbounded path search or A*.
fn swim_navigation<S: BlockSampler>(sampler: &S, body: EntityBody, wanted: Vec3) -> Vec3 {
    for candidate in [
        wanted,
        Vec3::new(wanted.x, 0.0, wanted.z),
        Vec3::new(0.0, wanted.y, 0.0),
        Vec3::new(-wanted.z, wanted.y, wanted.x),
        Vec3::new(wanted.z, wanted.y, -wanted.x),
        Vec3::new(-wanted.x, -wanted.y, -wanted.z),
    ] {
        if candidate != Vec3::ZERO && water_path(sampler, body, candidate) {
            return candidate;
        }
    }
    Vec3::ZERO
}

fn rotate(current: f32, wanted: f32, limit: f32) -> f32 {
    let difference = (wanted - current + 180.0).rem_euclid(360.0) - 180.0;
    current + difference.clamp(-limit, limit)
}

pub(crate) fn look(
    rotation: [f32; 3],
    position: Vec3,
    velocity: Vec3,
    target: Option<[f64; 3]>,
) -> [f32; 3] {
    let direction = target.map_or(velocity, |target| {
        Vec3::new(
            target[0] - position.x,
            target[1] - position.y,
            target[2] - position.z,
        )
    });
    let horizontal = mth::length2(direction.x, direction.z);
    if horizontal <= EPSILON && direction.y.abs() <= EPSILON {
        return rotation;
    }
    let yaw = if horizontal > EPSILON {
        rotate(
            rotation[0],
            -(mth::atan2(direction.x, direction.z) as f32) * mth::RAD_TO_DEG,
            90.0,
        )
    } else {
        rotation[0]
    };
    let pitch = rotate(
        rotation[1],
        -(mth::atan2(direction.y, horizontal) as f32) * mth::RAD_TO_DEG,
        40.0,
    )
    .clamp(-90.0, 90.0);
    [yaw, pitch, yaw]
}

pub(crate) fn step<S: BlockSampler>(
    input: MotionInput,
    sampler: &S,
    loaded: impl Fn(i32, i32) -> bool,
) -> MotionResult {
    let mut body = input.body;
    let mut impulse = input.impulse;
    if !loaded(floor_i32(body.position.x), floor_i32(body.position.z)) {
        body.velocity = Vec3::ZERO;
        return MotionResult {
            body,
            impulse: [0.0; 2],
            rotation: input.rotation,
            air_supply: input.air_supply,
            drowning: false,
        };
    }
    let water = matches!(
        input.movement,
        MovementClass::Swimmer | MovementClass::Amphibious
    ) && water_body(sampler, body.position, body.aabb);
    let (air_supply, drowning) = if input.aquatic_control == AquaticControl::Dolphin {
        let eye = Vec3::new(
            body.position.x,
            body.position.y + body.aabb.height * 0.9,
            body.position.z,
        );
        crate::dolphin::breathe(
            input.air_supply,
            sampler.material_at(floor_i32(eye.x), floor_i32(eye.y), floor_i32(eye.z))
                == BlockMaterial::Water,
        )
    } else {
        breathe(input.air_supply, water, input.movement)
    };
    if input.movement == MovementClass::Stationary {
        body.velocity = Vec3::ZERO;
        return MotionResult {
            body,
            impulse: [0.0; 2],
            rotation: look(input.rotation, body.position, Vec3::ZERO, input.look_target),
            air_supply,
            drowning,
        };
    }
    let mut config = PhysicsConfig::living_entity();
    // AI locomotion is a standing velocity, not decaying combat momentum.
    config.air_drag = 1.0;
    config.ground_friction = 1.0;
    config.vertical_air_drag = 0.98;
    if input.climbs_walls {
        config.jump_speed = 0.0;
    }
    body.velocity.x = input.drive[0] + impulse[0];
    body.velocity.z = input.drive[2] + impulse[1];
    if input.movement == MovementClass::Flyer
        || water
        || (input.aquatic_control == AquaticControl::Dolphin
            && sampler.material_at(
                floor_i32(body.position.x),
                floor_i32(body.position.y),
                floor_i32(body.position.z),
            ) == BlockMaterial::Water)
    {
        config.gravity = 0.0;
        config.jump_speed = 0.0;
        config.step_height = 0.0;
        config.water_buoyancy = 0.0;
        config.water_drag = 1.0;
        config.vertical_air_drag = 1.0;
        body.velocity.y = input.drive[1];
        if water && input.aquatic_control != AquaticControl::Dolphin {
            body.velocity = swim_navigation(sampler, body, body.velocity);
        }
    } else if input.movement == MovementClass::Swimmer {
        config.jump_speed = 0.0;
        config.step_height = 0.0;
        match input.aquatic_control {
            AquaticControl::Fish if body.on_ground => {
                body.velocity.x = (unit(input.id, input.tick) * 2.0 - 1.0) * 0.05 * 20.0;
                body.velocity.z =
                    (unit(input.id, input.tick.wrapping_add(1)) * 2.0 - 1.0) * 0.05 * 20.0;
                body.velocity.y = 0.400_000_005_960_464_5 * 20.0;
            }
            _ => {
                body.velocity.x = impulse[0];
                body.velocity.z = impulse[1];
            }
        }
    }
    if input.levitation > 0.0 {
        config.gravity = 0.0;
        body.velocity.y += (f64::from(input.levitation) - body.velocity.y) * 0.2;
    } else if input.slow_falling
        && body.velocity.y <= 0.0
        && input.movement != MovementClass::Flyer
        && !water
    {
        config.gravity = 4.0;
    }
    let requested = body.velocity;
    let mut resolved = mc_physics::step_entity(body, sampler, config);
    // Spiders climb only against a real horizontal obstacle; the same swept
    // collision resolver clips climb at ceilings and missing chunk columns.
    if input.climbs_walls
        && resolved.horizontal_collision
        && !water
        && loaded(
            floor_i32(
                body.position.x
                    + requested.x * mc_physics::TICK_SECONDS
                    + requested.x.signum() * body.aabb.half_width,
            ),
            floor_i32(
                body.position.z
                    + requested.z * mc_physics::TICK_SECONDS
                    + requested.z.signum() * body.aabb.half_width,
            ),
        )
    {
        let climb = (body.position.y + 0.2 - resolved.body.position.y).max(0.0);
        if let Some(rise) = mc_physics::resolve_collision_displacement(
            sampler,
            resolved.body.position,
            body.aabb,
            Vec3::new(0.0, climb, 0.0),
            false,
            0.0,
        ) {
            resolved.body.position.y += rise.delta.y;
            resolved.body.velocity.y =
                (resolved.body.position.y - body.position.y) / mc_physics::TICK_SECONDS;
            resolved.body.on_ground = false;
        }
    }
    body = resolved.body;
    for (axis, wanted, actual) in [
        (0, requested.x, body.velocity.x),
        (1, requested.z, body.velocity.z),
    ] {
        if actual.abs() < EPSILON && wanted.abs() > EPSILON {
            impulse[axis] = 0.0;
        } else {
            let drag = if water {
                0.9
            } else if body.on_ground {
                0.91 * 0.6
            } else {
                0.91
            };
            let next = impulse[axis] * drag;
            let next = if next.abs() < 0.06 { 0.0 } else { next };
            if axis == 0 {
                body.velocity.x += next - impulse[axis];
            } else {
                body.velocity.z += next - impulse[axis];
            }
            impulse[axis] = next;
        }
    }
    let rotation = look(
        input.rotation,
        body.position,
        body.velocity,
        input.look_target,
    );
    MotionResult {
        body,
        impulse,
        rotation,
        air_supply,
        drowning,
    }
}
