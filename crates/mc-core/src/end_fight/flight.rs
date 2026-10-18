use super::*;
use crate::projectiles::{ProjectileData, ProjectileKind, ProjectileState};
use crate::survival::DamageKind;
use mc_physics::mth;

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}
fn perched(phase: Phase) -> bool {
    matches!(
        phase,
        Phase::PerchingScanning | Phase::PerchingAttacking | Phase::PerchingFlaming
    )
}
fn nearest(state: &TickState, from: [f64; 3]) -> Option<(Uuid, [f64; 3])> {
    state
        .players
        .iter()
        .filter(|(_, p)| !p.creative() && !p.spectator() && !p.vitals.is_dead())
        .filter_map(|(id, _)| state.entities.slot_of(*id))
        .map(|s| (state.entities.row(s).uuid, state.entities.pos(s)))
        .filter(|(_, at)| distance(from, *at) < 150.0 * 150.0)
        .min_by(|a, b| {
            distance(from, a.1)
                .total_cmp(&distance(from, b.1))
                .then_with(|| a.0.cmp(&b.0))
        })
}
fn target(state: &TickState) -> Option<[f64; 3]> {
    let slot = state.entities.slot_of_uuid(state.end_fight.target?)?;
    let player = state.players.get(&state.entities.body(slot).id)?;
    (!player.creative() && !player.spectator() && !player.vitals.is_dead())
        .then(|| state.entities.pos(slot))
}
fn orbit(cycle: u32) -> [f64; 3] {
    let angle = f64::from(cycle % 12) * std::f64::consts::TAU / 12.0;
    [
        f64::from(mth::cos(angle)) * 60.0,
        96.0,
        f64::from(mth::sin(angle)) * 60.0,
    ]
}
pub(super) fn run(state: &mut TickState, intents: &mut Vec<Intent>, slot: EntitySlot) {
    if state.entities.row(slot).controls.no_ai {
        return;
    }
    let id = state.entities.body(slot).id;
    let at = state.entities.pos(slot);
    let [x, y, z] = state.end_fight.exit_portal;
    let fountain = [f64::from(x) + 0.5, f64::from(y + 4), f64::from(z) + 0.5];
    state.end_fight.phase_ticks = state.end_fight.phase_ticks.saturating_add(1);
    let time = state.end_fight.phase_ticks;
    match state.end_fight.phase {
        Phase::Circling => {
            if distance(at, state.end_fight.fly_target) < 100.0 || time == 1 {
                state.end_fight.cycle = state.end_fight.cycle.wrapping_add(1);
                state.end_fight.fly_target = orbit(state.end_fight.cycle);
                if time > 200 {
                    if let Some((uuid, position)) = nearest(state, at) {
                        state.end_fight.target = Some(uuid);
                        match state.end_fight.cycle % 4 {
                            0 => {
                                state.end_fight.phase(Phase::LandingApproach);
                                state.end_fight.fly_target = [40.0, 85.0, 0.0];
                            }
                            1 | 2 => {
                                state.end_fight.phase(Phase::Strafing);
                                state.end_fight.fly_target =
                                    [position[0], position[1] + 10.0, position[2]];
                            }
                            _ => {
                                state.end_fight.phase(Phase::Charging);
                                state.end_fight.fly_target = position;
                            }
                        }
                    } else {
                        state.end_fight.phase(Phase::LandingApproach);
                        state.end_fight.fly_target = [40.0, 85.0, 0.0];
                    }
                }
            }
        }
        Phase::Strafing => {
            if let Some(mut position) = target(state) {
                position[1] += 1.62;
                if distance(at, position) < 64.0 * 64.0
                    && crate::hostile::sight_clear(state, at, position)
                {
                    state.end_fight.fireball_charge = (state.end_fight.fireball_charge + 1).min(5);
                } else {
                    state.end_fight.fireball_charge =
                        state.end_fight.fireball_charge.saturating_sub(1);
                }
                if state.end_fight.fireball_charge >= 5 {
                    let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
                    let dx = position[0] - at[0];
                    let dz = position[2] - at[2];
                    let facing = ((f64::from(mth::sin(yaw)) * dx - f64::from(mth::cos(yaw)) * dz)
                        / mth::length2(dx, dz).max(0.001))
                    .clamp(-1.0, 1.0)
                    .acos()
                    .to_degrees();
                    if facing < 10.0 {
                        let from = [
                            at[0] + f64::from(mth::sin(yaw)) * 6.5,
                            at[1] + 2.0,
                            at[2] - f64::from(mth::cos(yaw)) * 6.5,
                        ];
                        let direction = std::array::from_fn(|i| position[i] - from[i]);
                        let length = direction
                            .iter()
                            .map(|v| v * v)
                            .sum::<f64>()
                            .sqrt()
                            .max(0.001);
                        let record = ProjectileState::new(
                            ProjectileKind::DragonFireball,
                            Some(state.entities.row(slot).uuid),
                            mc_data::ItemStack::EMPTY,
                        );
                        crate::projectiles::launch(
                            state,
                            record,
                            from,
                            direction.map(|v| v / length * 0.1),
                        );
                        state.end_fight.phase(Phase::Circling);
                    }
                }
                if time > 200 {
                    state.end_fight.phase(Phase::Circling);
                }
            } else {
                state.end_fight.phase(Phase::Circling);
            }
        }
        Phase::LandingApproach => {
            if distance(at, state.end_fight.fly_target) < 100.0 {
                state.end_fight.phase(Phase::Landing);
                state.end_fight.fly_target = fountain;
            }
        }
        Phase::Landing => {
            if distance(at, fountain) < 1.0 {
                state.end_fight.phase(Phase::PerchingScanning);
                state.end_fight.perched_damage = 0.0;
                state.end_fight.flame_count = 0;
            }
        }
        Phase::PerchingScanning => {
            if let Some((uuid, position)) = nearest(state, fountain) {
                let yaw =
                    mth::atan2(position[0] - at[0], at[2] - position[2]) as f32 * mth::RAD_TO_DEG;
                state.entities.set_rot(slot, yaw, 0.0);
                state.end_fight.target = Some(uuid);
                if distance(fountain, position) < 20.0 * 20.0 && time >= 25 {
                    state.end_fight.phase(Phase::PerchingAttacking);
                } else if time >= 100 {
                    state.end_fight.phase(Phase::Charging);
                    state.end_fight.fly_target = position;
                }
            } else if time >= 100 {
                state.end_fight.phase(Phase::Takeoff);
                state.end_fight.fly_target = [40.0, 100.0, 0.0];
            }
        }
        Phase::PerchingAttacking => {
            if time >= 40 {
                state.end_fight.phase(Phase::PerchingFlaming);
                state.end_fight.flame_count += 1;
            }
        }
        Phase::PerchingFlaming => {
            if time == 10 {
                let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
                let position = [
                    at[0] + f64::from(mth::sin(yaw)) * 8.0,
                    f64::from(y + 1),
                    at[2] - f64::from(mth::cos(yaw)) * 8.0,
                ];
                let mut record = ProjectileState::new(
                    ProjectileKind::AreaEffectCloud,
                    Some(state.entities.row(slot).uuid),
                    mc_data::ItemStack::EMPTY,
                );
                let mut cloud = crate::projectiles::payloads::CloudState::dragon(state.tick);
                cloud.radius = 5.0;
                cloud.radius_per_tick = 0.0;
                cloud.duration = 200;
                cloud.expires_tick = state.tick + 200;
                record.data = ProjectileData::Cloud(cloud);
                state.end_fight.flame_cloud =
                    crate::projectiles::launch(state, record, position, [0.0; 3]).and_then(|id| {
                        state
                            .entities
                            .slot_of(id)
                            .map(|slot| state.entities.row(slot).uuid)
                    });
            }
            if time >= 200 {
                if state.end_fight.flame_count >= 4 {
                    state.end_fight.phase(Phase::Takeoff);
                    state.end_fight.fly_target = [40.0, 100.0, 0.0];
                } else {
                    state.end_fight.phase(Phase::PerchingScanning);
                }
            }
        }
        Phase::Takeoff => {
            if distance(at, fountain) > 100.0 {
                state.end_fight.phase(Phase::Circling);
                state.end_fight.fly_target = orbit(state.end_fight.cycle);
            }
        }
        Phase::Charging => {
            if time >= 10 && (distance(at, state.end_fight.fly_target) < 100.0 || time > 100) {
                state.end_fight.phase(Phase::Circling);
                state.end_fight.fly_target = orbit(state.end_fight.cycle);
            }
        }
        Phase::Dying | Phase::Hover => {}
    }
    let phase = state.end_fight.phase;
    if !perched(phase) && phase != Phase::Dying {
        steer(state, slot, phase);
    } else {
        state.entities.set_vel(slot, [0.0; 3]);
    }
    state.end_fight.flight_history.copy_within(0..16, 1);
    state.end_fight.flight_history[0] = [
        state.entities.pos(slot)[1],
        f64::from(state.entities.rot(slot)[0]),
    ];
    metadata(state, slot, 16, EntityDataValue::VarInt(phase as i32));
    heal(state, slot);
    contact(state, intents, id);
    if crate::command_rules::boolean(state, "mob_griefing") {
        break_blocks(state, intents, id);
    }
}
fn heal(state: &mut TickState, slot: EntitySlot) {
    if state.tick.is_multiple_of(10) {
        let at = state.entities.pos(slot);
        state.end_fight.healing_crystal = state
            .end_fight
            .crystals
            .iter()
            .filter(|c| distance(at, c.position) < 32.0 * 32.0)
            .min_by(|a, b| {
                distance(at, a.position)
                    .total_cmp(&distance(at, b.position))
                    .then_with(|| a.uuid.cmp(&b.uuid))
            })
            .map(|c| c.uuid);
        if state.end_fight.healing_crystal.is_some() {
            let row = state.entities.row_mut(slot);
            row.health = (row.health + 1.0).min(200.0);
            std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
            state.appearance_changed.push(state.entities.body(slot).id);
        }
    }
}
/// Child identities are protocol-derived, never independent durable entities.
pub(crate) fn attack_target(state: &TickState, target: EntityId) -> (EntityId, bool) {
    let Some(slot) = dragon_slot(state) else {
        return (target, false);
    };
    let id = state.entities.body(slot).id;
    let offset = target.0 - id.0;
    if (1..=8).contains(&offset) {
        (id, offset != 1)
    } else {
        (target, false)
    }
}
pub(crate) fn part_bounds(state: &TickState, target: EntityId) -> Option<([f64; 3], [f64; 3])> {
    let slot = dragon_slot(state)?;
    let id = state.entities.body(slot).id;
    let offset = target.0 - id.0;
    if !(1..=8).contains(&offset) {
        return None;
    }
    let at = state.entities.pos(slot);
    let yaw = f64::from(state.entities.rot(slot)[0]).to_radians();
    let history = &state.end_fight.flight_history;
    let tilt = ((history[5][0] - history[10][0]) as f32 * 10.0).to_radians();
    let (sin_tilt, cos_tilt) = (
        mc_physics::mth::sin(f64::from(tilt)),
        mc_physics::mth::cos(f64::from(tilt)),
    );
    let (width, height) = match offset {
        1 => (1.0, 1.0),
        2 => (3.0, 3.0),
        3 => (5.0, 3.0),
        4..=6 => (2.0, 2.0),
        _ => (4.0, 2.0),
    };
    let (sin_yaw, cos_yaw) = (mc_physics::mth::sin(yaw), mc_physics::mth::cos(yaw));
    let delta = match offset {
        1 | 2 => {
            let length = if offset == 1 { 6.5 } else { 5.5 };
            let adjusted = yaw - f64::from(state.end_fight.yaw_accel * 0.01);
            [
                f64::from(mc_physics::mth::sin(adjusted) * length * cos_tilt),
                history[5][0] - history[0][0] + f64::from(sin_tilt * length),
                -f64::from(mc_physics::mth::cos(adjusted) * length * cos_tilt),
            ]
        }
        3 => [f64::from(sin_yaw * 0.5), 0.0, -f64::from(cos_yaw * 0.5)],
        4..=6 => {
            let index = (offset - 4) as usize;
            let p0 = history[12 + index * 2];
            let p1 = history[5];
            let rot = yaw + f64::from(wrap((p0[1] - p1[1]) as f32).to_radians());
            let length = (index + 1) as f32 * 2.0;
            [
                -f64::from((sin_yaw * 1.5 + mc_physics::mth::sin(rot) * length) * cos_tilt),
                p0[0] - p1[0] - f64::from((length + 1.5) * sin_tilt) + 1.5,
                f64::from((cos_yaw * 1.5 + mc_physics::mth::cos(rot) * length) * cos_tilt),
            ]
        }
        _ => {
            let side = if offset == 7 { 4.5 } else { -4.5 };
            [f64::from(cos_yaw * side), 2.0, f64::from(sin_yaw * side)]
        }
    };
    let center: [f64; 3] = std::array::from_fn(|i| at[i] + delta[i]);
    Some((
        [center[0] - width / 2.0, center[1], center[2] - width / 2.0],
        [
            center[0] + width / 2.0,
            center[1] + height,
            center[2] + width / 2.0,
        ],
    ))
}
fn contact(state: &mut TickState, intents: &mut Vec<Intent>, dragon: EntityId) {
    let victims: Vec<_> = state
        .players
        .iter()
        .filter(|(_, p)| !p.creative() && !p.spectator() && !p.vitals.is_dead())
        .filter_map(|(id, _)| {
            state.entities.slot_of(*id).map(|s| {
                (
                    *id,
                    state.entities.pos(s),
                    state.entities.body(s).half_width,
                    state.entities.body(s).height,
                )
            })
        })
        .collect();
    for (id, at, width, height) in victims {
        for part in [1, 2, 7, 8] {
            let Some((min, max)) = part_bounds(state, EntityId(dragon.0 + part)) else {
                continue;
            };
            if at[0] + width >= min[0]
                && at[0] - width <= max[0]
                && at[1] + height >= min[1]
                && at[1] <= max[1]
                && at[2] + width >= min[2]
                && at[2] - width <= max[2]
            {
                let seq = state.next_seq(dragon);
                intents.push(Intent {
                    actor: dragon,
                    seq,
                    kind: IntentKind::DamageEntity {
                        target: id,
                        amount: if part <= 2 { 10.0 } else { 5.0 },
                        kind: DamageKind::Melee,
                        source: Some(crate::intent::DamageSource::Melee {
                            attacker: dragon,
                            position: state.entities.pos(dragon_slot(state).unwrap()),
                            yaw: state.entities.rot(dragon_slot(state).unwrap())[0],
                            bonus_knockback: 1.0,
                            sweep: false,
                            fire_ticks: 0,
                            bane_ticks: 0,
                        }),
                    },
                    footprint: Footprint {
                        entity_writes: vec![id],
                        ..Default::default()
                    },
                });
                break;
            }
        }
    }
}
fn break_blocks(state: &mut TickState, intents: &mut Vec<Intent>, dragon: EntityId) {
    for part in 1..=3 {
        let Some((min, max)) = part_bounds(state, EntityId(dragon.0 + part)) else {
            continue;
        };
        for x in min[0].floor() as i32..=max[0].floor() as i32 {
            for y in min[1].floor() as i32..=max[1].floor() as i32 {
                for z in min[2].floor() as i32..=max[2].floor() as i32 {
                    let Some(block) = state.world.block_at(x, y, z) else {
                        continue;
                    };
                    let Some(info) = state.registry.blocks.by_id(mc_world::BlockStateId(block))
                    else {
                        continue;
                    };
                    if state.registry.tags.contains_raw_id(
                        "minecraft:block",
                        "minecraft:dragon_transparent",
                        info.block.raw_id,
                    ) || state.registry.tags.contains_raw_id(
                        "minecraft:block",
                        "minecraft:dragon_immune",
                        info.block.raw_id,
                    ) || matches!(
                        info.block.id.path(),
                        "air"
                            | "cave_air"
                            | "void_air"
                            | "bedrock"
                            | "obsidian"
                            | "end_stone"
                            | "iron_bars"
                            | "end_portal"
                            | "end_portal_frame"
                            | "end_gateway"
                    ) {
                        continue;
                    }
                    let pos = crate::boundary::BlockPos { x, y, z };
                    let seq = state.next_seq(dragon);
                    intents.push(Intent {
                        actor: dragon,
                        seq,
                        kind: IntentKind::UseBlock { pos, hand: None },
                        footprint: Footprint {
                            block_expect: vec![(pos, block)],
                            block_write: vec![(pos, state.registry.states.air)],
                            ..Default::default()
                        },
                    });
                }
            }
        }
    }
}

fn wrap(mut degrees: f32) -> f32 {
    degrees %= 360.0;
    if degrees >= 180.0 {
        degrees -= 360.0;
    }
    if degrees < -180.0 {
        degrees += 360.0;
    }
    degrees
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| a * b).sum()
}
fn normalized(v: [f64; 3]) -> [f64; 3] {
    let length = dot(v, v).sqrt();
    if length <= 1.0e-7 {
        [0.0; 3]
    } else {
        v.map(|v| v / length)
    }
}
/// Port of the OLD 26.1.2 air kernel; row velocity uses blocks/second.
fn steer(state: &mut TickState, slot: EntitySlot, phase: Phase) {
    let at = state.entities.pos(slot);
    let target = state.end_fight.fly_target;
    let delta: [f64; 3] = std::array::from_fn(|i| target[i] - at[i]);
    let distance_sq = dot(delta, delta);
    if distance_sq <= 1.0e-14 {
        state.entities.set_vel(slot, [0.0; 3]);
        return;
    }
    let velocity = state.entities.vel(slot).map(|v| v / 20.0);
    let mut next = velocity;
    let speed = match phase {
        Phase::Charging => 3.0,
        Phase::Landing => 1.5,
        _ => 0.6,
    };
    let horizontal = mth::length2(delta[0], delta[2]);
    next[1] += if horizontal > 0.0 {
        (delta[1] / horizontal).clamp(-speed, speed)
    } else {
        delta[1]
    } * 0.01;
    let mut yaw = wrap(state.entities.rot(slot)[0]);
    let yaw_rad = f64::from(yaw * mth::DEG_TO_RAD);
    let aim = normalized(delta);
    let direction = normalized([
        f64::from(mth::sin(yaw_rad)),
        next[1],
        -f64::from(mth::cos(yaw_rad)),
    ]);
    let alignment = ((dot(direction, aim) + 0.5) / 1.5).max(0.0);
    if delta[0].abs() > 1.0e-5 || delta[2].abs() > 1.0e-5 {
        let change = wrap(180.0 - mth::atan2(delta[0], delta[2]) as f32 * mth::RAD_TO_DEG - yaw)
            .clamp(-50.0, 50.0);
        let adjusted = mth::length2(velocity[0], velocity[2]) + 1.0;
        // DragonLandingPhase overrides the default turn coefficient.
        let turn = if phase == Phase::Landing {
            let adjusted = adjusted as f32;
            adjusted.min(40.0) / adjusted
        } else {
            (0.7 / adjusted.min(40.0) / adjusted) as f32
        };
        state.end_fight.yaw_accel = state.end_fight.yaw_accel * 0.8 + change * turn;
        yaw = wrap(yaw + state.end_fight.yaw_accel * 0.1);
    }
    let span = 2.0 / (distance_sq + 1.0);
    let thrust = 0.06 * (alignment * span + (1.0 - span));
    let yaw_rad = f64::from(yaw * mth::DEG_TO_RAD);
    next[0] += f64::from(mth::sin(yaw_rad)) * thrust;
    next[2] -= f64::from(mth::cos(yaw_rad)) * thrust;
    let slide = 0.8 + 0.15 * (dot(normalized(next), direction) + 1.0) / 2.0;
    next[0] *= slide;
    next[1] *= 0.91;
    next[2] *= slide;
    state
        .entities
        .set_pos(slot, std::array::from_fn(|i| at[i] + next[i]));
    state.entities.set_vel(slot, next.map(|v| v * 20.0));
    state.entities.set_rot(slot, yaw, 0.0);
}
