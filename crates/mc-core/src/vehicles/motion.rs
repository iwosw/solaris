use super::VehicleKind;
use crate::boundary::BlockPos;
use crate::entities::EntitySlot;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::tick::{TickSampler, TickState};
use mc_physics::mth;
use mc_physics::{Aabb, Vec3};
use mc_world::BlockStateId;

#[derive(Clone, Copy)]
pub(super) struct Rail {
    pub cell: BlockPos,
    pub ends: [[f64; 3]; 2],
    pub powered: bool,
    pub boost: bool,
    pub activator: bool,
}
impl Rail {
    pub fn point(self, t: f64) -> [f64; 3] {
        std::array::from_fn(|i| self.ends[0][i] + (self.ends[1][i] - self.ends[0][i]) * t)
    }
    fn tangent(self) -> [f64; 3] {
        let d: [f64; 3] = std::array::from_fn(|i| self.ends[1][i] - self.ends[0][i]);
        let length = mth::length2(d[0], d[2]);
        d.map(|v| v / length)
    }
    fn project(self, p: [f64; 3]) -> f64 {
        let d: [f64; 3] = std::array::from_fn(|i| self.ends[1][i] - self.ends[0][i]);
        ((p[0] - self.ends[0][0]) * d[0] + (p[2] - self.ends[0][2]) * d[2])
            / (d[0] * d[0] + d[2] * d[2])
    }
}
fn property<'a>(b: &'a mc_world::BlockState, key: &str) -> Option<&'a str> {
    b.properties
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}
pub(super) fn rail_at(state: &TickState, cell: BlockPos) -> Option<Rail> {
    let block = state
        .registry
        .blocks
        .by_id(BlockStateId(state.world.block_at(cell.x, cell.y, cell.z)?))?;
    let name = block.block.id.path();
    if !matches!(
        name,
        "rail" | "powered_rail" | "detector_rail" | "activator_rail"
    ) {
        return None;
    }
    let (a, b) = match property(block, "shape")? {
        "north_south" => ([0.5, 0.0, 0.0], [0.5, 0.0, 1.0]),
        "east_west" => ([0.0, 0.0, 0.5], [1.0, 0.0, 0.5]),
        "ascending_east" => ([0.0, 0.0, 0.5], [1.0, 1.0, 0.5]),
        "ascending_west" => ([0.0, 1.0, 0.5], [1.0, 0.0, 0.5]),
        "ascending_north" => ([0.5, 1.0, 0.0], [0.5, 0.0, 1.0]),
        "ascending_south" => ([0.5, 0.0, 0.0], [0.5, 1.0, 1.0]),
        "south_east" => ([0.5, 0.0, 1.0], [1.0, 0.0, 0.5]),
        "south_west" => ([0.5, 0.0, 1.0], [0.0, 0.0, 0.5]),
        "north_west" => ([0.5, 0.0, 0.0], [0.0, 0.0, 0.5]),
        "north_east" => ([0.5, 0.0, 0.0], [1.0, 0.0, 0.5]),
        _ => return None,
    };
    let origin = [
        f64::from(cell.x),
        f64::from(cell.y) + 0.0625,
        f64::from(cell.z),
    ];
    Some(Rail {
        cell,
        ends: [
            std::array::from_fn(|i| origin[i] + a[i]),
            std::array::from_fn(|i| origin[i] + b[i]),
        ],
        powered: property(block, "powered") == Some("true"),
        boost: name == "powered_rail",
        activator: name == "activator_rail",
    })
}
fn current_rail(state: &TickState, p: [f64; 3]) -> Option<Rail> {
    let mut best = None;
    let mut best_distance = 1.1;
    for y in [p[1].floor() as i32, p[1].floor() as i32 - 1] {
        if let Some(r) = rail_at(
            state,
            BlockPos {
                x: p[0].floor() as i32,
                y,
                z: p[2].floor() as i32,
            },
        ) {
            let q = r.point(r.project(p).clamp(0.0, 1.0));
            let distance = (q[1] - p[1]).abs();
            if distance < best_distance {
                best = Some(r);
                best_distance = distance;
            }
        }
    }
    best
}
fn connected(state: &TickState, rail: Rail, endpoint: usize) -> Option<(Rail, usize)> {
    let p = rail.ends[endpoint];
    let dx = if (p[0] - f64::from(rail.cell.x)).abs() < 1e-7 {
        -1
    } else if (p[0] - f64::from(rail.cell.x) - 1.0).abs() < 1e-7 {
        1
    } else {
        0
    };
    let dz = if (p[2] - f64::from(rail.cell.z)).abs() < 1e-7 {
        -1
    } else if (p[2] - f64::from(rail.cell.z) - 1.0).abs() < 1e-7 {
        1
    } else {
        0
    };
    for y in [rail.cell.y, rail.cell.y + 1, rail.cell.y - 1] {
        if let Some(next) = rail_at(
            state,
            BlockPos {
                x: rail.cell.x + dx,
                y,
                z: rail.cell.z + dz,
            },
        ) {
            for e in 0..2 {
                if next.ends[e]
                    .into_iter()
                    .zip(p)
                    .all(|(a, b)| (a - b).abs() < 1e-6)
                {
                    return Some((next, e));
                }
            }
        }
    }
    None
}
pub(super) fn clear(state: &TickState, pos: [f64; 3], half_width: f64, height: f64) -> bool {
    if pos.iter().any(|v| !v.is_finite()) {
        return false;
    }
    let body = [
        pos[0] - half_width,
        pos[1],
        pos[2] - half_width,
        pos[0] + half_width,
        pos[1] + height,
        pos[2] + half_width,
    ];
    for x in (body[0].floor() as i32)..=(body[3].floor() as i32) {
        for y in (body[1].floor() as i32 - 1)..=(body[4].floor() as i32) {
            for z in (body[2].floor() as i32)..=(body[5].floor() as i32) {
                let Some(id) = state.world.block_at(x, y, z) else {
                    return false;
                };
                let Some(boxes) = state.registry.collision_boxes.get(id as usize) else {
                    return false;
                };
                for b in boxes {
                    let local = b.as_blocks();
                    let obstacle = [
                        local[0] + f64::from(x),
                        local[1] + f64::from(y),
                        local[2] + f64::from(z),
                        local[3] + f64::from(x),
                        local[4] + f64::from(y),
                        local[5] + f64::from(z),
                    ];
                    if mc_physics::aabb_intersects_deflated_obstacle(body, obstacle, 1e-7) {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn collide(
    state: &mut TickState,
    slot: EntitySlot,
    desired: [f64; 3],
    velocity: [f64; 3],
    rail: bool,
) -> bool {
    let p = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let delta = Vec3::new(desired[0] - p[0], desired[1] - p[1], desired[2] - p[2]);
    let Some(resolved) = mc_physics::resolve_collision_displacement(
        &TickSampler {
            world: &state.world,
            registry: &state.registry,
        },
        Vec3::new(p[0], p[1], p[2]),
        Aabb {
            half_width: body.half_width,
            height: body.height,
        },
        delta,
        state.entities.ground(slot),
        if rail && delta.y > 0.0 { 1.0 } else { 0.0 },
    ) else {
        return false;
    };
    let actual = [resolved.delta.x, resolved.delta.y, resolved.delta.z];
    let clipped = (actual[0] - delta.x).abs() > 1e-6 || (actual[2] - delta.z).abs() > 1e-6;
    state
        .entities
        .set_pos(slot, std::array::from_fn(|i| p[i] + actual[i]));
    let mut velocity = velocity;
    for axis in 0..3 {
        if (actual[axis] - [delta.x, delta.y, delta.z][axis]).abs() > 1e-6 {
            velocity[axis] = 0.0;
        }
    }
    state.entities.set_vel(slot, velocity.map(|v| v * 20.0));
    state.entities.set_ground(slot, resolved.grounded || rail);
    !clipped
}
fn boat(state: &mut TickState, slot: EntitySlot) {
    let p = state.entities.pos(slot);
    let input = crate::mounts::vehicle_input(state, slot);
    let mut yaw = state.entities.rot(slot)[0];
    let left = input & 4 != 0;
    let right = input & 8 != 0;
    let forward = input & 1 != 0;
    let back = input & 2 != 0;
    yaw += (i32::from(right) - i32::from(left)) as f32 * 4.0;
    let angle = f64::from(yaw).to_radians();
    let mut vel = state.entities.vel(slot).map(|v| v / 20.0);
    let water = state
        .world
        .block_at(
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        )
        .and_then(|id| state.registry.blocks.by_id(BlockStateId(id)))
        .is_some_and(|b| b.block.id.path() == "water");
    let acceleration = if forward {
        0.04
    } else if back {
        -0.005
    } else if left != right {
        0.005
    } else {
        0.0
    };
    let drag = if water {
        0.9
    } else if state.entities.ground(slot) {
        0.5
    } else {
        0.9
    };
    vel[0] = vel[0] * drag - f64::from(mth::sin(angle)) * acceleration;
    vel[2] = vel[2] * drag + f64::from(mth::cos(angle)) * acceleration;
    vel[1] = if water {
        ((f64::from(p[1].floor() as i32) + 0.8 - p[1]) * 0.1).clamp(-0.05, 0.05)
    } else {
        (vel[1] - 0.04) * 0.98
    };
    let speed = mth::length2(vel[0], vel[2]);
    if speed > 0.4 {
        vel[0] *= 0.4 / speed;
        vel[2] *= 0.4 / speed;
    }
    let desired = std::array::from_fn(|i| p[i] + vel[i]);
    collide(state, slot, desired, vel, false);
    state.entities.set_rot(slot, yaw, 0.0);
    let paddles = [forward || right, forward || left];
    if state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .paddles
        != paddles
    {
        state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .paddles = paddles;
    }
}
fn cart(state: &mut TickState, slot: EntitySlot) {
    let p = state.entities.pos(slot);
    let old = state.entities.vel(slot).map(|v| v / 20.0);
    let Some(mut rail) = current_rail(state, p) else {
        let mut vel = old;
        vel[1] -= 0.04;
        if state.entities.ground(slot) {
            vel[0] *= 0.5;
            vel[2] *= 0.5;
        }
        collide(
            state,
            slot,
            std::array::from_fn(|i| p[i] + vel[i]),
            vel.map(|v| v * 0.95),
            false,
        );
        return;
    };
    let tangent = rail.tangent();
    let mut speed = old[0] * tangent[0] + old[2] * tangent[2];
    speed -= tangent[1] * 0.0078125;
    let input = crate::mounts::vehicle_input(state, slot);
    if speed.abs() < 0.01
        && input & 1 != 0
        && let Some(rider) = crate::mounts::rider(state, slot)
    {
        let yaw = f64::from(state.entities.rot(rider)[0]).to_radians();
        speed +=
            (-f64::from(mth::sin(yaw)) * tangent[0] + f64::from(mth::cos(yaw)) * tangent[2]) * 0.1;
    }
    let v = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap();
    if v.fuel > 0 {
        let along = v.push[0] * tangent[0] + v.push[1] * tangent[2];
        if along.abs() > 1e-6 {
            speed += along.signum() * 0.04;
        }
    }
    if rail.boost {
        if rail.powered {
            if speed.abs() > 0.01 {
                speed += speed.signum() * 0.06;
            } else {
                // A solid block at one end launches a stationary cart away from it.
                for endpoint in 0..2 {
                    let end = rail.ends[endpoint];
                    let direction = if endpoint == 0 { -1.0 } else { 1.0 };
                    let sample = [
                        end[0] + tangent[0] * direction * 0.5,
                        end[1],
                        end[2] + tangent[2] * direction * 0.5,
                    ];
                    if state
                        .world
                        .block_at(
                            sample[0].floor() as i32,
                            sample[1].floor() as i32,
                            sample[2].floor() as i32,
                        )
                        .is_some_and(|id| {
                            state
                                .registry
                                .collision_boxes
                                .get(id as usize)
                                .is_some_and(|b| {
                                    b.contains(&mc_physics::BlockCollisionBox::FULL_BLOCK)
                                })
                        })
                    {
                        speed = -direction * 0.02;
                        break;
                    }
                }
            }
        } else {
            speed = if speed.abs() < 0.03 { 0.0 } else { speed * 0.5 };
        }
    }
    let max = if v.fuel > 0 { 0.2 } else { 0.4 };
    speed = speed.clamp(-max, max);
    if rail.activator && rail.powered {
        let name = super::name(state, slot);
        let can_prime = crate::command_rules::boolean(state, "tnt_explodes");
        let v = state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap();
        if name == Some("minecraft:tnt_minecart") {
            if v.fuse < 0 && can_prime {
                v.fuse = 80;
            }
        } else if name == Some("minecraft:hopper_minecart") {
            v.enabled = false;
        }
        if let Some(rider) = crate::mounts::rider(state, slot) {
            let id = state.entities.body(rider).id;
            crate::mounts::dismount(state, id);
        }
    } else if super::name(state, slot) == Some("minecraft:hopper_minecart") && rail.activator {
        state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap()
            .enabled = true;
    }
    let mut t = rail.project(p).clamp(0.0, 1.0);
    let mut remaining = speed.abs();
    let mut sign = speed.signum();
    // A capped 0.4-block move can cross at most two rail chords; bound corrupt geometry too.
    for _ in 0..4 {
        let length = mth::length2(
            rail.ends[1][0] - rail.ends[0][0],
            rail.ends[1][2] - rail.ends[0][2],
        );
        let endpoint = usize::from(sign >= 0.0);
        let to_end = if endpoint == 1 {
            (1.0 - t) * length
        } else {
            t * length
        };
        if remaining <= to_end || remaining < 1e-8 {
            t += sign * remaining / length;
            remaining = 0.0;
            break;
        }
        remaining -= to_end;
        if let Some((next, entry)) = connected(state, rail, endpoint) {
            rail = next;
            t = entry as f64;
            sign = if entry == 0 { 1.0 } else { -1.0 };
        } else {
            t = endpoint as f64;
            break;
        }
    }
    let tangent = rail.tangent();
    let mut desired = rail.point(t);
    if remaining > 0.0 {
        desired[0] += tangent[0] * sign * remaining;
        desired[2] += tangent[2] * sign * remaining;
    }
    let drag = if crate::mounts::rider(state, slot).is_some() {
        0.997
    } else {
        0.96
    };
    let velocity = [
        tangent[0] * sign * speed.abs() * drag,
        0.0,
        tangent[2] * sign * speed.abs() * drag,
    ];
    let moved = collide(state, slot, desired, velocity, true);
    if moved && speed.abs() > 0.001 {
        let yaw = mth::atan2(-velocity[0], velocity[2]) as f32 * mth::RAD_TO_DEG;
        state.entities.set_rot(slot, yaw, 0.0);
    }
    if !moved
        && super::name(state, slot) == Some("minecraft:tnt_minecart")
        && mth::length2(old[0], old[2]) > 0.1
    {
        let v = state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap();
        if v.fuse < 0 {
            v.fuse = 0;
        }
    }
}
fn entity_collisions(state: &mut TickState, slot: EntitySlot) {
    let p = state.entities.pos(slot);
    let body = state.entities.body(slot);
    let width = body.half_width;
    let height = body.height;
    let rider = crate::mounts::rider(state, slot);
    for i in 0..state.entities.len() {
        let other = EntitySlot(i as u32);
        if other == slot || Some(other) == rider {
            continue;
        }
        let body = state.entities.body(other);
        if !body.alive
            || matches!(
                body.kind,
                crate::entities::EntityKind::Item { .. }
                    | crate::entities::EntityKind::ExperienceOrb { .. }
            )
        {
            continue;
        }
        let q = state.entities.pos(other);
        let dx = q[0] - p[0];
        let dz = q[2] - p[2];
        if dx.abs() > width + body.half_width
            || dz.abs() > width + body.half_width
            || q[1] >= p[1] + height
            || p[1] >= q[1] + body.height
        {
            continue;
        }
        let length = mth::length2(dx, dz);
        if length < 1e-6 {
            continue;
        }
        let impulse = [dx / length * 0.05, dz / length * 0.05];
        let mut a = state.entities.vel(slot);
        let mut b = state.entities.vel(other);
        a[0] -= impulse[0] * 20.0;
        a[2] -= impulse[1] * 20.0;
        b[0] += impulse[0] * 20.0;
        b[2] += impulse[1] * 20.0;
        state.entities.set_vel(slot, a);
        state.entities.set_vel(other, b);
        let prev = state.entities.horizontal_impulse(other);
        state.entities.set_horizontal_impulse(
            other,
            [prev[0] + impulse[0] * 20.0, prev[1] + impulse[1] * 20.0],
        );
    }
}
pub(super) fn step(state: &mut TickState, slot: EntitySlot, intents: &mut Vec<Intent>) {
    let p = state.entities.pos(slot);
    if state
        .world
        .block_at(
            p[0].floor() as i32,
            p[1].floor() as i32,
            p[2].floor() as i32,
        )
        .is_none()
    {
        return;
    }
    match super::kind(state, slot) {
        Some(VehicleKind::Boat) => boat(state, slot),
        Some(VehicleKind::Minecart) => cart(state, slot),
        None => return,
    }
    entity_collisions(state, slot);
    let velocity = state.entities.vel(slot);
    let ground = state.entities.ground(slot);
    let v = state
        .entities
        .row(slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap();
    let fuel = v.fuel.saturating_sub(1);
    let push = if fuel == 0 { [0.0; 2] } else { v.push };
    let fuse = if v.fuse > 0 { v.fuse - 1 } else { v.fuse };
    if (v.on_ground, v.fuel, v.push, v.fuse) != (ground, fuel, push, fuse) {
        let v = state
            .entities
            .row_mut(slot)
            .entity_life
            .vehicle
            .as_mut()
            .unwrap();
        v.on_ground = ground;
        v.fuel = fuel;
        v.push = push;
        v.fuse = fuse;
    }
    if fuse == 0 {
        if !crate::command_rules::boolean(state, "tnt_explodes") {
            state
                .entities
                .row_mut(slot)
                .entity_life
                .vehicle
                .as_mut()
                .unwrap()
                .fuse = -1;
            return;
        }
        let v = state
            .entities
            .row(slot)
            .entity_life
            .vehicle
            .as_ref()
            .unwrap();
        let power = v.explosion_power
            + v.explosion_speed_factor
                * (mth::length2(velocity[0], velocity[2]) / 20.0).min(5.0) as f32
                * 1.5;
        let id = state.entities.body(slot).id;
        intents.push(Intent {
            actor: id,
            seq: state.next_seq(id),
            kind: IntentKind::Explode {
                center: state.entities.pos(slot),
                power,
                mob_source: false,
                kill_credit: None,
            },
            footprint: Footprint::default(),
        });
        state.entities.body_mut(slot).alive = false;
        state.entities.row_mut(slot).loot.death_handled = true;
    }
}
