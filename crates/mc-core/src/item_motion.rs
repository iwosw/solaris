//! ItemEntity motion: retain launch momentum through air, fluids, and collision.

use mc_physics::mth;
use mc_physics::{BlockMaterial, BlockSampler, EntityBody, Vec3};

pub(crate) fn step<S: BlockSampler>(mut body: EntityBody, sampler: &S) -> EntityBody {
    let material = sampler.material_at(
        body.position.x.floor() as i32,
        (body.position.y + 0.1).floor() as i32,
        body.position.z.floor() as i32,
    );
    // ItemEntity.tick / setFluidMovement (26.1.2): velocities here are blocks/s.
    match material {
        BlockMaterial::Water | BlockMaterial::Lava => {
            let drag = if material == BlockMaterial::Water {
                0.99
            } else {
                0.95
            };
            body.velocity.x *= drag;
            body.velocity.z *= drag;
            if body.velocity.y < 0.06 * 20.0 {
                body.velocity.y += 0.0005 * 20.0;
            }
        }
        _ => body.velocity.y -= 0.04 * 20.0,
    }
    let requested = Vec3::new(
        body.velocity.x * mc_physics::TICK_SECONDS,
        body.velocity.y * mc_physics::TICK_SECONDS,
        body.velocity.z * mc_physics::TICK_SECONDS,
    );
    let Some(resolved) = mc_physics::resolve_collision_displacement(
        sampler,
        body.position,
        body.aabb,
        requested,
        body.on_ground,
        0.0,
    ) else {
        body.velocity = Vec3::ZERO;
        return body;
    };
    body.position.x += resolved.delta.x;
    body.position.y += resolved.delta.y;
    body.position.z += resolved.delta.z;
    body.on_ground = resolved.grounded;
    let horizontal_drag = if body.on_ground { 0.6 * 0.98 } else { 0.98 };
    body.velocity.x = if (resolved.delta.x - requested.x).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.x * horizontal_drag
    };
    body.velocity.y = if (resolved.delta.y - requested.y).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.y * 0.98
    };
    body.velocity.z = if (resolved.delta.z - requested.z).abs() > 1e-7 {
        0.0
    } else {
        body.velocity.z * horizontal_drag
    };
    body
}

pub(crate) fn throw_selected(
    state: &mut crate::tick::TickState,
    actor: crate::boundary::EntityId,
    drops: &[crate::boundary::EntityId],
) {
    let Some(actor_slot) = state.entities.slot_of(actor) else {
        return;
    };
    let mut pos = state.entities.pos(actor_slot);
    let rotation = state.entities.rot(actor_slot);
    let eye = crate::tick::player_eye_height(state.entities.row(actor_slot).appearance.pose);
    pos[1] += eye - 0.3;
    let yaw = f64::from(rotation[0]).to_radians();
    let pitch = f64::from(rotation[1]).to_radians();
    let velocity = [
        -f64::from(mth::sin(yaw)) * f64::from(mth::cos(pitch)) * 6.0,
        -f64::from(mth::sin(pitch)) * 6.0 + 2.0,
        f64::from(mth::cos(yaw)) * f64::from(mth::cos(pitch)) * 6.0,
    ];
    for id in drops {
        if let Some(slot) = state.entities.slot_of(*id) {
            state.entities.set_pos(slot, pos);
            state.entities.set_vel(slot, velocity);
            state.entities.body_mut(slot).pickup_delay = 40;
        }
    }
}
