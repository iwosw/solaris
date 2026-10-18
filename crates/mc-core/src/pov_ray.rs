//! Item POV clipping: vanilla ClipContext.Block.OUTLINE, not collision/full voxels.

use mc_data::collision_shapes::CollisionShape;
use mc_physics::mth;

use crate::boundary::BlockPos;
use crate::tick::TickState;

const REACH: f64 = 4.5;

#[derive(Clone, Copy, Default)]
pub(crate) struct OutlineState {
    pub shape: Option<CollisionShape<'static>>,
    pub horizontal: f64,
}

impl OutlineState {
    fn offset(self, pos: BlockPos) -> [f64; 3] {
        if self.horizontal == 0.0 {
            return [0.0; 3];
        }
        // BlockBehaviour.Properties.offsetType / Mth.getSeed, javap 26.1.2.
        // X multiplies as an int before widening; all subsequent arithmetic wraps long.
        let seed =
            i64::from(pos.x.wrapping_mul(3_129_871)) ^ i64::from(pos.z).wrapping_mul(116_129_781);
        let seed = seed
            .wrapping_mul(seed)
            .wrapping_mul(42_317_861)
            .wrapping_add(seed.wrapping_mul(11))
            >> 16;
        let fraction = |shift: u32| f64::from(((seed >> shift) & 15_i64) as f32 / 15.0);
        [
            ((fraction(0) - 0.5) * 0.5).clamp(-self.horizontal, self.horizontal),
            0.0,
            ((fraction(8) - 0.5) * 0.5).clamp(-self.horizontal, self.horizontal),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Hit {
    pub pos: BlockPos,
    pub face: [i32; 3],
}

/// Both bucket and bottle UseItem resolve the same authoritative item POV ray.
/// Filled buckets pass `false`; empty buckets and bottles use SOURCE_ONLY fluids.
pub(crate) fn clip(
    state: &TickState,
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    source_fluids: bool,
) -> Option<Hit> {
    let (pitch, yaw) = (pitch.to_radians(), (-yaw).to_radians());
    let dir = [
        f64::from(mth::sin(f64::from(yaw)) * mth::cos(f64::from(pitch))),
        f64::from(-mth::sin(f64::from(pitch))),
        f64::from(mth::cos(f64::from(yaw)) * mth::cos(f64::from(pitch))),
    ];
    let mut cell = eye.map(|value| value.floor() as i32);
    let mut steps = [0; 3];
    let mut next = [f64::INFINITY; 3];
    let mut delta = [f64::INFINITY; 3];
    for axis in 0..3 {
        if dir[axis] > 0.0 {
            steps[axis] = 1;
            next[axis] = (f64::from(cell[axis]) + 1.0 - eye[axis]) / dir[axis];
            delta[axis] = 1.0 / dir[axis];
        } else if dir[axis] < 0.0 {
            steps[axis] = -1;
            next[axis] = (f64::from(cell[axis]) - eye[axis]) / dir[axis];
            delta[axis] = 1.0 / -dir[axis];
        }
    }
    loop {
        let pos = BlockPos {
            x: cell[0],
            y: cell[1],
            z: cell[2],
        };
        let block = state.world.block_at(pos.x, pos.y, pos.z)?;
        let outline = *state.registry.outlines.get(block as usize)?;
        let translation = outline.offset(pos);
        let local_eye =
            std::array::from_fn(|axis| eye[axis] - f64::from(cell[axis]) - translation[axis]);
        let mut nearest = None;
        if let Some(shape) = outline.shape {
            for bounds in shape.iter().map(|bounds| bounds.as_blocks()) {
                retain_nearest(&mut nearest, clip_box(local_eye, dir, bounds));
            }
        }
        if source_fluids && let Some((fluid, 0)) = crate::fluids::contained(state, block) {
            let above_same = state
                .world
                .block_at(pos.x, pos.y + 1, pos.z)
                .and_then(|above| crate::fluids::contained(state, above))
                .is_some_and(|(above, _)| above == fluid);
            let height = if above_same {
                1.0
            } else {
                f64::from(8.0_f32 / 9.0)
            };
            let local_eye = std::array::from_fn(|axis| eye[axis] - f64::from(cell[axis]));
            retain_nearest(
                &mut nearest,
                clip_box(local_eye, dir, [0.0, 0.0, 0.0, 1.0, height, 1.0]),
            );
        }
        if let Some((_, face)) = nearest {
            return Some(Hit { pos, face });
        }
        let axis = if next[0] < next[1] {
            if next[0] < next[2] { 0 } else { 2 }
        } else if next[1] < next[2] {
            1
        } else {
            2
        };
        if next[axis] > REACH {
            return None;
        }
        cell[axis] += steps[axis];
        next[axis] += delta[axis];
    }
}

fn retain_nearest(nearest: &mut Option<(f64, [i32; 3])>, candidate: Option<(f64, [i32; 3])>) {
    if let Some(candidate) = candidate
        && nearest.is_none_or(|current| candidate.0 < current.0)
    {
        *nearest = Some(candidate);
    }
}

fn clip_box(eye: [f64; 3], dir: [f64; 3], bounds: [f64; 6]) -> Option<(f64, [i32; 3])> {
    // VoxelShape.clip first probes 0.001 of the complete ray for an inside hit.
    let probe = std::array::from_fn::<_, 3, _>(|axis| eye[axis] + dir[axis] * REACH * 0.001);
    if (0..3).all(|axis| probe[axis] >= bounds[axis] && probe[axis] < bounds[axis + 3]) {
        let mut axis = 1;
        for other in [2, 0] {
            if dir[other].abs() > dir[axis].abs() {
                axis = other;
            }
        }
        let mut face = [0; 3];
        face[axis] = if dir[axis] > 0.0 { -1 } else { 1 };
        return Some((REACH * 0.001, face));
    }
    let mut enter: f64 = 0.0;
    let mut exit = REACH;
    let mut face = [0; 3];
    for axis in 0..3 {
        if dir[axis].abs() < 1.0e-7 {
            if eye[axis] < bounds[axis] - 1.0e-7 || eye[axis] > bounds[axis + 3] + 1.0e-7 {
                return None;
            }
            continue;
        }
        let (near, far, normal) = if dir[axis] > 0.0 {
            (
                (bounds[axis] - eye[axis]) / dir[axis],
                (bounds[axis + 3] - eye[axis]) / dir[axis],
                -1,
            )
        } else {
            (
                (bounds[axis + 3] - eye[axis]) / dir[axis],
                (bounds[axis] - eye[axis]) / dir[axis],
                1,
            )
        };
        if near > enter {
            enter = near;
            face = [0; 3];
            face[axis] = normal;
        }
        exit = exit.min(far);
        if enter > exit {
            return None;
        }
    }
    (enter > 0.0 && enter < REACH && exit > 0.0).then_some((enter, face))
}

/// Actual selection-offset users reported by the 26.1.2 outline extractor.
/// Short/tall grass and fern model offsets do not move their selection shapes.
pub(crate) fn offset_bounds(name: &str) -> f64 {
    match name {
        "minecraft:mangrove_propagule"
        | "minecraft:dandelion"
        | "minecraft:golden_dandelion"
        | "minecraft:torchflower"
        | "minecraft:poppy"
        | "minecraft:blue_orchid"
        | "minecraft:allium"
        | "minecraft:azure_bluet"
        | "minecraft:red_tulip"
        | "minecraft:orange_tulip"
        | "minecraft:white_tulip"
        | "minecraft:pink_tulip"
        | "minecraft:oxeye_daisy"
        | "minecraft:cornflower"
        | "minecraft:wither_rose"
        | "minecraft:lily_of_the_valley"
        | "minecraft:bamboo_sapling"
        | "minecraft:bamboo"
        | "minecraft:open_eyeblossom"
        | "minecraft:closed_eyeblossom" => 0.25,
        "minecraft:pointed_dripstone" => 0.125,
        _ => 0.0,
    }
}
