//! Shared swept collision, ballistic motion, pickup and loyalty return.

use std::sync::Arc;

use mc_physics::BlockMaterial;
use uuid::Uuid;

use super::{ARROW_IN_GROUND_INDEX, ARROW_STUCK_TICKS, ProjectileKind, ProjectileState};
use crate::boundary::{EntityDataValue, EntityId};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{DamageSource, Footprint, Intent, IntentKind};
use crate::survival::DamageKind;
use crate::tick::TickState;

#[derive(Debug)]
pub(super) struct Hittable {
    id: EntityId,
    uuid: Uuid,
    deployed_candidate: Option<EntitySlot>,
    min: [f64; 3],
    max: [f64; 3],
}

fn hittables(state: &TickState, bodies: &mut Vec<Hittable>) {
    bodies.clear();
    bodies.extend(state.entities.slots().filter_map(|slot| {
        let body = state.entities.body(slot);
        let row = state.entities.row(slot);
        if crate::end_fight::is_dragon(state, slot) {
            return None;
        }
        let hittable = body.alive
            && match body.kind {
                EntityKind::Player { .. } => state
                    .players
                    .get(&body.id)
                    .is_some_and(|player| !player.creative() && !player.spectator()),
                EntityKind::Mob { .. } => {
                    body.movement != crate::blueprint::MovementClass::Projectile
                }
                EntityKind::Item { .. } | EntityKind::ExperienceOrb { .. } => false,
            };
        hittable.then(|| {
            let pos = state.entities.pos(slot);
            Hittable {
                id: body.id,
                uuid: row.uuid,
                deployed_candidate: crate::armor_stand::is_stand(state, slot).then_some(slot),
                min: [pos[0] - body.half_width, pos[1], pos[2] - body.half_width],
                max: [
                    pos[0] + body.half_width,
                    pos[1] + body.height,
                    pos[2] + body.half_width,
                ],
            }
        })
    }));
    if let Some(uuid) = state.end_fight.dragon_uuid
        && let Some(slot) = state.entities.slot_of_uuid(uuid)
    {
        let id = state.entities.body(slot).id;
        for offset in 1..=8 {
            let part = EntityId(id.0 + offset);
            if let Some((min, max)) = crate::end_fight::part_bounds(state, part) {
                bodies.push(Hittable {
                    id: part,
                    uuid,
                    deployed_candidate: None,
                    min,
                    max,
                });
            }
        }
    }
}

pub(super) fn segment_enters(
    origin: [f64; 3],
    delta: [f64; 3],
    min: [f64; 3],
    max: [f64; 3],
) -> Option<f64> {
    let (mut near, mut far) = (0.0_f64, 1.0_f64);
    for axis in 0..3 {
        if delta[axis].abs() < 1.0e-9 {
            if origin[axis] < min[axis] || origin[axis] > max[axis] {
                return None;
            }
        } else {
            let a = (min[axis] - origin[axis]) / delta[axis];
            let b = (max[axis] - origin[axis]) / delta[axis];
            near = near.max(a.min(b));
            far = far.min(a.max(b));
            if near > far {
                return None;
            }
        }
    }
    Some(near)
}

/// Exact swept sphere against an axis-aligned box, including rounded corners.
/// Each box-face crossing partitions squared distance into one quadratic.
pub(crate) fn swept_sphere_enters(
    origin: [f64; 3],
    delta: [f64; 3],
    min: [f64; 3],
    max: [f64; 3],
    radius: f64,
) -> Option<f64> {
    if radius == 0.0 {
        return segment_enters(origin, delta, min, max);
    }
    if !radius.is_finite()
        || radius < 0.0
        || origin
            .iter()
            .chain(delta.iter())
            .chain(min.iter())
            .chain(max.iter())
            .any(|v| !v.is_finite())
        || (0..3).any(|axis| min[axis] > max[axis])
    {
        return None;
    }
    let mut cuts = [0.0; 8];
    cuts[1] = 1.0;
    let mut count = 2;
    for axis in 0..3 {
        if delta[axis] == 0.0 {
            continue;
        }
        for face in [min[axis], max[axis]] {
            let crossing = (face - origin[axis]) / delta[axis];
            if crossing > 0.0 && crossing < 1.0 {
                cuts[count] = crossing;
                count += 1;
            }
        }
    }
    cuts[..count].sort_unstable_by(f64::total_cmp);
    for pair in cuts[..count].windows(2) {
        let (start, end) = (pair[0], pair[1]);
        if start == end {
            continue;
        }
        let middle = (start + end) * 0.5;
        let (mut quadratic, mut linear, mut constant) = (0.0, 0.0, -radius * radius);
        for axis in 0..3 {
            let at = origin[axis] + delta[axis] * middle;
            let face = if at < min[axis] {
                min[axis]
            } else if at > max[axis] {
                max[axis]
            } else {
                continue;
            };
            let offset = origin[axis] - face;
            quadratic += delta[axis] * delta[axis];
            linear += 2.0 * offset * delta[axis];
            constant += offset * offset;
        }
        if (quadratic * start + linear) * start + constant <= 0.0 {
            return Some(start);
        }
        if quadratic == 0.0 {
            continue;
        }
        let discriminant = linear * linear - 4.0 * quadratic * constant;
        if discriminant < 0.0 {
            continue;
        }
        let entry = (-linear - discriminant.sqrt()) / (2.0 * quadratic);
        if entry >= start && entry <= end {
            return Some(entry);
        }
    }
    None
}

/// Swept ray against the same exact registry shapes as walker collision.
/// Unloaded cells pause flight instead of deleting retained ammunition.
fn first_solid(
    state: &TickState,
    from: [f64; 3],
    step: [f64; 3],
    radius: f64,
) -> Result<Option<(f64, crate::boundary::BlockPos)>, ()> {
    if from
        .iter()
        .chain(step.iter())
        .any(|value| !value.is_finite())
        || step.iter().any(|value| value.abs() > 128.0)
        || !radius.is_finite()
        || !(0.0..=128.0).contains(&radius)
    {
        return Err(());
    }
    let end: [f64; 3] = std::array::from_fn(|axis| from[axis] + step[axis]);
    let min: [i32; 3] =
        std::array::from_fn(|axis| (from[axis].min(end[axis]) - radius).floor() as i32);
    let max: [i32; 3] =
        std::array::from_fn(|axis| (from[axis].max(end[axis]) + radius).floor() as i32);
    let below = i32::from(
        state
            .registry
            .max_collision_box_y
            .div_ceil(16)
            .saturating_sub(1),
    );
    let mut first: Option<(f64, crate::boundary::BlockPos)> = None;
    for y in min[1].saturating_sub(below)..=max[1] {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                let block = state.world.block_at(x, y, z).ok_or(())?;
                let Some(boxes) = state.registry.collision_boxes.get(block as usize) else {
                    return Err(());
                };
                for collision_box in boxes {
                    let local = collision_box.as_blocks();
                    let offset = [f64::from(x), f64::from(y), f64::from(z)];
                    let lower = std::array::from_fn(|axis| offset[axis] + local[axis]);
                    let upper = std::array::from_fn(|axis| offset[axis] + local[axis + 3]);
                    if let Some(t) = swept_sphere_enters(from, step, lower, upper, radius)
                        && first.is_none_or(|(old, _)| t < old)
                    {
                        first = Some((t, crate::boundary::BlockPos { x, y, z }));
                    }
                }
            }
        }
    }
    Ok(first)
}

/// Physical melee shares native block shapes and refuses unloaded segments.
pub(crate) fn clear_segment(state: &TickState, start: [f64; 3], end: [f64; 3]) -> bool {
    let step = std::array::from_fn(|axis| end[axis] - start[axis]);
    matches!(first_solid(state, start, step, 0.0), Ok(None))
}

pub(super) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    ids: impl Iterator<Item = EntityId>,
) {
    state.projectiles.retired_crossbows.clear();
    let mut ids = ids.peekable();
    if ids.peek().is_none() {
        return;
    }
    let mut bodies = std::mem::take(&mut state.projectiles.bodies);
    hittables(state, &mut bodies);
    for id in ids {
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let ppos = state.entities.pos(slot);
        if !state.world.is_active(
            (ppos[0].floor() as i32).div_euclid(16),
            (ppos[2].floor() as i32).div_euclid(16),
        ) {
            continue;
        }
        let Some(mut boxed) = state.entities.row_mut(slot).projectile.take() else {
            // Vanilla /summon projectile: give it a native retained flight record.
            let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
                continue;
            };
            let Some(kind) = state
                .registry
                .blueprint(type_id)
                .and_then(|blueprint| ProjectileKind::from_name(blueprint.name))
            else {
                continue;
            };
            state.entities.row_mut(slot).projectile = Some(Box::new(ProjectileState::new(
                kind,
                None,
                mc_data::ItemStack::EMPTY,
            )));
            super::payloads::initialize(state, id);
            super::touch(state, id);
            continue;
        };
        let record = &mut *boxed;
        record.age = record.age.saturating_add(1);
        let gone = step(state, intents, id, record, &bodies);
        if gone {
            // Removing the row must not discard actual weapon custody before
            // the lethal-hit hooks run during same-tick damage admission.
            if record.fired_from_weapon.is_some()
                && !state.registry.advancements.definitions.is_empty()
            {
                state.projectiles.retired_crossbows.insert(id, *boxed);
            }
            super::remove(state, id);
        } else if let Some(slot) = state.entities.slot_of(id) {
            super::initialize_appearance(state, slot, record);
            state.entities.row_mut(slot).projectile = Some(boxed);
            super::touch(state, id);
        }
    }
    state.projectiles.bodies = bodies;
}

/// Attached rockets follow their player's UUID, never a restart-local entity id.
/// Missing/dead players detach; client-side boost does not use terrain collision.
fn attached(state: &mut TickState, id: EntityId, record: &mut ProjectileState) -> bool {
    let Some(target) = record.attached_to else {
        return false;
    };
    let Some(target_slot) = state.entities.slot_of_uuid(target) else {
        record.attached_to = None;
        super::payloads::clear_attachment(state, id);
        return false;
    };
    if !state.entities.body(target_slot).alive {
        record.attached_to = None;
        super::payloads::clear_attachment(state, id);
        return false;
    }
    let Some(slot) = state.entities.slot_of(id) else {
        return true;
    };
    state
        .entities
        .set_pos(slot, state.entities.pos(target_slot));
    true
}
fn arrow_damage(
    state: &TickState,
    id: EntityId,
    target: EntityId,
    record: &ProjectileState,
    velocity: [f64; 3],
) -> f32 {
    let speed = velocity.iter().map(|v| v * v).sum::<f64>().sqrt();
    let mut amount = if record.kind == ProjectileKind::Trident {
        record.damage + crate::enchantments::target_damage(state, target, &record.stack)
    } else {
        (speed * f64::from(record.damage)).ceil() as f32
    };
    if record.critical {
        let mut rng = crate::natural_spawn::Rng(state.tick ^ id.0 as u64 ^ target.0 as u64);
        amount += (rng.next() % ((amount / 2.0 + 2.0) as u64).max(1)) as f32;
    }
    if record.mob_damage {
        amount = state.difficulty.scale_mob_damage(amount);
    }
    amount
}

fn step(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &mut ProjectileState,
    bodies: &[Hittable],
) -> bool {
    let is_attached = record.kind == ProjectileKind::Firework
        && record.attached_to.is_some()
        && attached(state, id, record);
    if super::payloads::step(state, intents, id, record) {
        return false;
    }
    if record.kind == ProjectileKind::FishingHook
        && super::fishing::step(state, intents, id, record)
    {
        return false;
    }
    if is_attached {
        return false;
    }
    let Some(slot) = state.entities.slot_of(id) else {
        return true;
    };
    let pos = state.entities.pos(slot);
    let authored = crate::physical_attacks::projectile_profile(state, slot);
    if matches!(record.data, super::ProjectileData::Physical { .. }) && authored.is_none() {
        crate::plugin::fail_native(state);
        return false;
    }
    if let Some(profile) = authored {
        record.pickup &= profile.recoverable;
    }
    if loyalty(state, id, slot, record, pos) {
        return state.entities.slot_of(id).is_none();
    }
    if record.stuck {
        if record.age > ARROW_STUCK_TICKS {
            return true;
        }
        return pickup(state, record, pos);
    }
    if record.age > authored.map_or(1200, |profile| profile.lifetime_ticks) {
        return true;
    }
    if let super::ProjectileData::Homing { target } = record.data
        && let Some(target_slot) = state.entities.slot_of_uuid(target)
        && state.entities.body(target_slot).alive
    {
        let mut at = state.entities.pos(target_slot);
        at[1] += state.entities.body(target_slot).height * 0.5;
        let aim = crate::special_mobs::homing_velocity(pos, at);
        let current = state.entities.vel(slot).map(|value| value / 20.0);
        state.entities.set_vel(
            slot,
            std::array::from_fn(|axis| (current[axis] + (aim[axis] - current[axis]) * 0.2) * 20.0),
        );
    }
    let velocity = state.entities.vel(slot).map(|value| value / 20.0);
    let collision_radius = authored.map_or(0.0, |profile| profile.collision_radius);
    let Ok(block_hit) = first_solid(state, pos, velocity, collision_radius) else {
        return false;
    };
    let reach = block_hit.map_or(1.0, |(t, _)| t);
    let owner = super::owner_id(state, record);
    if !(record.kind == ProjectileKind::Trident && (!record.hit.is_empty() || record.returning)) {
        loop {
            let victim = bodies
                .iter()
                .filter(|body| Some(body.uuid) != record.owner && !record.hit.contains(&body.uuid))
                .filter(|body| {
                    owner.is_none_or(|owner| {
                        crate::scoreboard::permits_player_damage(state, owner, body.id)
                    })
                })
                .filter_map(|body| {
                    let hit = body.deployed_candidate.and_then(|candidate| {
                        crate::plugin_npcs::deployed_projectile_hit(
                            state,
                            candidate,
                            pos,
                            std::array::from_fn(|axis| pos[axis] + velocity[axis]),
                            authored.map_or(state.entities.body(slot).half_width, |profile| {
                                profile.collision_radius
                            }),
                        )
                    });
                    hit.unwrap_or_else(|| {
                        if let Some(profile) = authored {
                            swept_sphere_enters(
                                pos,
                                velocity,
                                body.min,
                                body.max,
                                profile.collision_radius,
                            )
                        } else {
                            segment_enters(
                                pos,
                                velocity,
                                body.min.map(|value| value - 0.3),
                                body.max.map(|value| value + 0.3),
                            )
                        }
                    })
                    .filter(|t| *t <= reach)
                    .map(|t| (t, body, hit.is_some()))
                })
                .min_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.id.cmp(&b.1.id)));
            let Some((t, body, deployed)) = victim else {
                break;
            };
            let impact = std::array::from_fn(|axis| pos[axis] + velocity[axis] * t);
            let physical_impact = if authored.is_some() {
                let physical = state.entities.slot_of_uuid(body.uuid).and_then(|target| {
                    crate::physical_attacks::projectile_hit(state, slot, target, impact)
                });
                let Some(physical) = physical else {
                    record.hit.push(body.uuid);
                    if record.kind == ProjectileKind::Trident {
                        state.entities.set_pos(slot, impact);
                        super::set_flight(state, slot, velocity.map(|value| value * -0.01));
                        return false;
                    }
                    if record.piercing == 0 || record.hit.len() > record.piercing as usize {
                        return true;
                    }
                    continue;
                };
                Some(physical)
            } else {
                None
            };
            if deployed && record.kind.arrow() {
                let candidate = body.deployed_candidate.expect("deployed native stand slot");
                let physical = physical_impact.unwrap_or_else(|| {
                    let direct_type_id = match state.entities.body(slot).kind {
                        EntityKind::Mob { type_id } => type_id,
                        _ => 0,
                    };
                    let piece = crate::tick::DamagePiece {
                        amount: arrow_damage(state, id, body.id, record, velocity),
                        kind: DamageKind::Projectile,
                        source: Some(DamageSource::Projectile {
                            projectile: id,
                            direct_type_id,
                            owner: owner.unwrap_or(EntityId(0)),
                            velocity: [velocity[0], velocity[2]],
                            punch: record.punch,
                            flame: record.flame,
                        }),
                        physical: None,
                        custom_armor: false,
                    };
                    let mut native =
                        crate::physical_defense::impact_from_piece(state, candidate, &piece)
                            .expect("native arrow physical impact");
                    native.point = impact;
                    native.collision_radius = state.entities.body(slot).half_width;
                    native
                });
                if let Some(blocked) =
                    crate::plugin_npcs::adjudicate_deployed_projectile(state, candidate, &physical)
                {
                    record.hit.push(body.uuid);
                    if blocked {
                        return true;
                    }
                    continue;
                }
            }
            if record.kind == ProjectileKind::FishingHook {
                super::fishing::impact(state, id, record, Some(body.id), impact);
                return false;
            }
            if record.kind.arrow() && crate::enderman::projectile_hit(state, body.id) {
                record.hit.push(body.uuid);
                state.entities.set_pos(slot, impact);
                super::set_flight(state, slot, velocity.map(|v| v * -0.1));
                record.critical = false;
                return false;
            }
            if !record.kind.arrow() {
                state.entities.set_pos(slot, impact);
                return super::payloads::impact(state, intents, id, record, Some(body.id), impact);
            }
            record.hit.push(body.uuid);
            if let Some(physical) = physical_impact {
                let seq = state.next_seq(id);
                let direct_type_id = match state.entities.body(slot).kind {
                    EntityKind::Mob { type_id } => type_id,
                    _ => 0,
                };
                intents.push(Intent {
                    actor: id,
                    seq,
                    kind: IntentKind::PhysicalDamageEntity {
                        target: body.id,
                        impact: physical,
                        source: Some(DamageSource::Projectile {
                            projectile: id,
                            direct_type_id,
                            owner: owner.unwrap_or(EntityId(0)),
                            velocity: [velocity[0], velocity[2]],
                            punch: record.punch,
                            flame: record.flame,
                        }),
                    },
                    footprint: Footprint {
                        entity_writes: vec![body.id],
                        ..Default::default()
                    },
                });
            } else {
                let amount = arrow_damage(state, id, body.id, record, velocity);
                super::damage(
                    state,
                    intents,
                    id,
                    body.id,
                    amount,
                    DamageKind::Projectile,
                    Some(owner.unwrap_or(EntityId(0))),
                    velocity,
                );
            }
            super::embedded_projectiles::queue_arrow_hit(state, id, body.id, record);
            if let Some(Intent {
                kind:
                    IntentKind::DamageEntity {
                        source: Some(DamageSource::Projectile { punch, flame, .. }),
                        ..
                    },
                ..
            }) = intents.last_mut()
            {
                *punch = record.punch;
                *flame = record.flame;
            }
            super::payloads::arrow_effects(state, intents, id, record, body.id);
            if record.kind == ProjectileKind::Trident {
                let _ = super::payloads::channeling(state, intents, id, record, impact);
                state.entities.set_pos(slot, impact);
                super::set_flight(state, slot, velocity.map(|v| v * -0.01));
                return false;
            }
            if record.piercing == 0 || record.hit.len() > record.piercing as usize {
                return true;
            }
        }
    }
    if let Some((t, block_position)) = block_hit {
        let impact = std::array::from_fn(|axis| pos[axis] + velocity[axis] * t);
        state.entities.set_pos(slot, impact);
        let owner = super::owner_id(state, record);
        crate::vibration::emit(
            state,
            impact,
            crate::vibration::GameEvent::ProjectileLand,
            owner,
        );
        if record.kind == ProjectileKind::FishingHook {
            super::fishing::impact(state, id, record, None, impact);
            return false;
        }
        if !record.kind.arrow() {
            return super::payloads::impact(state, intents, id, record, None, impact);
        }
        if record.kind != ProjectileKind::Trident
            && (record.flame || state.entities.body(slot).burn_ticks != 0)
            && crate::tnt::prime_block(state, block_position, 80).is_some()
        {
            return true;
        }
        record.stuck = true;
        record.critical = false;
        record.age = 0;
        state.entities.set_vel(slot, [0.0; 3]);
        state.entities.set_ground(slot, true);
        let extra = &mut Arc::make_mut(&mut state.entities.row_mut(slot).appearance).extra;
        if let Some((_, value)) = extra
            .iter_mut()
            .find(|(index, _)| *index == ARROW_IN_GROUND_INDEX)
        {
            *value = EntityDataValue::Boolean(true);
        } else {
            extra.push((ARROW_IN_GROUND_INDEX, EntityDataValue::Boolean(true)));
        }
        return false;
    }
    state
        .entities
        .set_pos(slot, std::array::from_fn(|axis| pos[axis] + velocity[axis]));
    if let Some(profile) = authored {
        let mut next = velocity;
        next[1] -= profile.gravity;
        super::set_flight(state, slot, next);
        return false;
    }
    let block = state.world.block_at(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let water =
        block.is_some_and(|block| state.registry.materials.classify(block) == BlockMaterial::Water);
    let (drag, gravity) = match record.kind {
        ProjectileKind::Arrow | ProjectileKind::SpectralArrow => {
            (if water { 0.6 } else { 0.99 }, 0.05)
        }
        ProjectileKind::Trident => (0.99, 0.05),
        ProjectileKind::Potion | ProjectileKind::LingeringPotion => (0.99, 0.05),
        ProjectileKind::ExperienceBottle => (0.99, 0.07),
        ProjectileKind::Firework => (1.0, 0.0),
        ProjectileKind::SmallFireball
        | ProjectileKind::Fireball
        | ProjectileKind::DragonFireball
        | ProjectileKind::WitherSkull => (if water { 0.8 } else { 0.95 }, 0.0),
        ProjectileKind::WindCharge
        | ProjectileKind::BreezeWindCharge
        | ProjectileKind::ShulkerBullet => (1.0, 0.0),
        _ => (if water { 0.8 } else { 0.99 }, 0.03),
    };
    let speed = velocity
        .iter()
        .map(|value| value * value)
        .sum::<f64>()
        .sqrt();
    let mut next = std::array::from_fn(|axis| {
        let acceleration = if speed > 1.0e-9 {
            velocity[axis] / speed * record.acceleration_power
        } else {
            0.0
        };
        (velocity[axis] + acceleration) * drag
    });
    next[1] -= gravity;
    if record.kind == ProjectileKind::Firework && !record.shot_at_angle {
        next[0] *= 1.15;
        next[2] *= 1.15;
        next[1] += 0.04;
    }
    super::set_flight(state, slot, next);
    false
}

fn pickup(state: &mut TickState, record: &ProjectileState, pos: [f64; 3]) -> bool {
    if !record.pickup || record.stack.is_empty() {
        return false;
    }
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        let player = body.id;
        if !body.alive
            || state
                .players
                .get(&player)
                .is_none_or(|player| player.spectator())
        {
            continue;
        }
        let at = state.entities.pos(slot);
        if at
            .iter()
            .zip(pos)
            .map(|(a, b)| (a - b).powi(2))
            .sum::<f64>()
            >= 2.25
        {
            continue;
        }
        if record.kind == ProjectileKind::Trident && super::owner_id(state, record) != Some(player)
        {
            continue;
        }
        if give(state, player, record.stack.clone()) {
            return true;
        }
    }
    false
}

fn give(state: &mut TickState, player: EntityId, mut stack: mc_data::ItemStack) -> bool {
    let rules = state.registry.item_rules();
    let Some(player) = state.players.get_mut(&player) else {
        return false;
    };
    player.inventory.add(&rules, &mut stack);
    if !stack.is_empty() {
        return false;
    }
    let connection = player.connection;
    let change = player.inventory_contents();
    state.push_player(connection, change);
    true
}

fn loyalty(
    state: &mut TickState,
    id: EntityId,
    slot: EntitySlot,
    record: &mut ProjectileState,
    pos: [f64; 3],
) -> bool {
    if record.kind != ProjectileKind::Trident {
        return false;
    }
    let level = record
        .stack
        .enchantments
        .iter()
        .filter(|enchantment| enchantment.id.as_str() == "minecraft:loyalty")
        .map(|enchantment| enchantment.level.max(0) as u32)
        .max()
        .unwrap_or(0);
    if level == 0 || (!record.stuck && record.hit.is_empty() && !record.returning) {
        return false;
    }
    let Some(owner) = super::owner_id(state, record) else {
        return false;
    };
    let Some(owner_slot) = state.entities.slot_of(owner) else {
        return false;
    };
    if state
        .players
        .get(&owner)
        .is_some_and(|player| player.spectator())
    {
        return false;
    }
    let target = state.entities.pos(owner_slot);
    let delta = [
        target[0] - pos[0],
        target[1] + 1.0 - pos[1],
        target[2] - pos[2],
    ];
    let distance = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
    record.returning = true;
    record.stuck = false;
    if distance < 1.5 {
        if !record.pickup || give(state, owner, record.stack.clone()) {
            super::remove(state, id);
        }
        return true;
    }
    let old = state.entities.vel(slot).map(|v| v / 20.0);
    let speed = 0.05 * f64::from(level);
    let velocity = std::array::from_fn(|axis| old[axis] * 0.95 + delta[axis] / distance * speed);
    state
        .entities
        .set_pos(slot, std::array::from_fn(|axis| pos[axis] + velocity[axis]));
    super::set_flight(state, slot, velocity);
    true
}
