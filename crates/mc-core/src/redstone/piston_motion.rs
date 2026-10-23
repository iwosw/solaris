//! Entity displacement for admitted piston transactions, using native collision shapes.
use crate::boundary::BlockPos;
use crate::tick::{TickSampler, TickState};
use mc_physics::{Aabb, BlockCollisionBox, BlockMaterial, BlockSampler, Vec3};
use mc_world::{BlockStateId, ChunkPos};

#[derive(Debug)]
pub(super) struct Motion {
    pub(super) step: (i32, i32, i32),
    pub(super) blocks: Vec<(BlockPos, BlockStateId)>,
}

struct Stationary<'a> {
    sampler: TickSampler<'a>,
    moving: &'a [(BlockPos, BlockStateId)],
}

impl Stationary<'_> {
    fn moving(&self, x: i32, y: i32, z: i32) -> bool {
        self.moving
            .iter()
            .any(|(pos, _)| *pos == BlockPos { x, y, z })
    }
}

impl BlockSampler for Stationary<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> BlockMaterial {
        if self.moving(x, y, z) {
            BlockMaterial::Air
        } else {
            self.sampler.material_at(x, y, z)
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        self.sampler.max_collision_box_y()
    }

    fn collision_boxes_at(&self, x: i32, y: i32, z: i32, emit: &mut dyn FnMut(BlockCollisionBox)) {
        if !self.moving(x, y, z) {
            self.sampler.collision_boxes_at(x, y, z, emit);
        }
    }
}

pub(super) fn apply(state: &mut TickState, motion: Motion) {
    let Some((first, _)) = motion.blocks.first() else {
        return;
    };
    let mut min_x = first.x;
    let mut max_x = first.x;
    let mut min_z = first.z;
    let mut max_z = first.z;
    for (pos, _) in &motion.blocks {
        min_x = min_x.min(pos.x);
        max_x = max_x.max(pos.x);
        min_z = min_z.min(pos.z);
        max_z = max_z.max(pos.z);
    }
    let margin = state.entities.max_half_width().ceil() as i32 + 1;
    let mut candidates = std::mem::take(&mut state.redstone.piston_entities);
    for x in (min_x - margin).div_euclid(16)..=(max_x + margin).div_euclid(16) {
        for z in (min_z - margin).div_euclid(16)..=(max_z + margin).div_euclid(16) {
            state
                .entities
                .for_each_slot_in_chunk(ChunkPos { x, z }, |slot| candidates.push(slot));
        }
    }
    candidates.sort_unstable_by_key(|slot| slot.0);
    candidates.dedup();
    let step = [
        f64::from(motion.step.0),
        f64::from(motion.step.1),
        f64::from(motion.step.2),
    ];
    let axis = step
        .iter()
        .position(|v| *v != 0.0)
        .expect("piston direction");
    for &slot in &candidates {
        let body = state.entities.body(slot);
        if state
            .players
            .get(&body.id)
            .is_some_and(|player| player.game_mode == crate::commands::GameMode::Spectator)
        {
            continue;
        }
        let position = state.entities.pos(slot);
        let aabb = Aabb {
            half_width: body.half_width,
            height: body.height,
        };
        let lo = [
            position[0] - aabb.half_width,
            position[1],
            position[2] - aabb.half_width,
        ];
        let hi = [
            position[0] + aabb.half_width,
            position[1] + aabb.height,
            position[2] + aabb.half_width,
        ];
        let mut distance = 0.0_f64;
        let mut slime = false;
        for (pos, id) in &motion.blocks {
            let Some(block) = state.registry.blocks.by_id(*id) else {
                continue;
            };
            let origin = [f64::from(pos.x), f64::from(pos.y), f64::from(pos.z)];
            for shape in &state.registry.collision_boxes[id.0 as usize] {
                let c = shape.as_blocks();
                let low: [f64; 3] = std::array::from_fn(|i| origin[i] + c[i]);
                let high: [f64; 3] = std::array::from_fn(|i| origin[i] + c[i + 3]);
                let intersects = (0..3).all(|i| {
                    hi[i] > low[i] + step[i].min(0.0) && lo[i] < high[i] + step[i].max(0.0)
                });
                if intersects {
                    let push = if step[axis] > 0.0 {
                        high[axis] + 1.0 - lo[axis]
                    } else {
                        hi[axis] - low[axis] + 1.0
                    };
                    distance = distance.max(push.min(1.0) + 0.01);
                    slime |= block.block.id.path() == "slime_block";
                }
                if block.block.id.path() == "honey_block"
                    && axis != 1
                    && state.entities.ground(slot)
                    && (position[1] - high[1]).abs() < 1.0e-5
                    && hi[0] > low[0]
                    && lo[0] < high[0]
                    && hi[2] > low[2]
                    && lo[2] < high[2]
                {
                    distance = distance.max(1.0);
                }
            }
        }
        if distance == 0.0 {
            continue;
        }
        let sampler = Stationary {
            sampler: TickSampler {
                world: &state.world,
                registry: &state.registry,
            },
            moving: &motion.blocks,
        };
        let desired = Vec3::new(step[0] * distance, step[1] * distance, step[2] * distance);
        let Some(resolved) = mc_physics::resolve_collision_displacement(
            &sampler,
            Vec3::new(position[0], position[1], position[2]),
            aabb,
            desired,
            state.entities.ground(slot),
            0.0,
        ) else {
            continue;
        };
        let new = [
            position[0] + resolved.delta.x,
            position[1] + resolved.delta.y,
            position[2] + resolved.delta.z,
        ];
        let id = body.id;
        state.entities.set_pos(slot, new);
        if slime {
            let mut velocity = state.entities.vel(slot);
            velocity[axis] = step[axis] / mc_physics::TICK_SECONDS;
            state.entities.set_vel(slot, velocity);
        }
        if let Some(player) = state.players.get(&id) {
            let connection = player.connection;
            let rotation = state.entities.rot(slot);
            state.correct_position(connection, new, rotation[0], rotation[1]);
        }
    }
    candidates.clear();
    state.redstone.piston_entities = candidates;
}
