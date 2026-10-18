use super::*;

pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    kind: GoalKind,
    targets: &[(EntityId, [f64; 3])],
    attack: &mut Attack,
) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let spos = state.entities.pos(slot);
    if !state.world.is_active(
        (spos[0].floor() as i32).div_euclid(16),
        (spos[2].floor() as i32).div_euclid(16),
    ) {
        return;
    }
    if kind == GoalKind::FangLife {
        let Some(&Retained::Fangs {
            owner,
            mut warmup,
            mut life,
        }) = state.entities.row(slot).special_mob.as_deref()
        else {
            return;
        };
        if warmup >= 0 {
            warmup -= 1;
        } else {
            if life == 22 {
                event(state, id, 4);
            }
            if life == 14 {
                let pos = state.entities.pos(slot);
                let owner_id = owner
                    .and_then(|uuid| state.entities.slot_of_uuid(uuid))
                    .map(|slot| state.entities.body(slot).id);
                let victims: Vec<EntityId> = state
                    .entities
                    .slots()
                    .filter_map(|victim| {
                        let row = state.entities.row(victim);
                        let at = state.entities.pos(victim);
                        (state.entities.body(victim).alive
                            && state.entities.body(victim).id != id
                            && Some(row.uuid) != owner
                            && state.entities.body(victim).movement
                                != crate::blueprint::MovementClass::Projectile
                            && !matches!(
                                state.entities.body(victim).kind,
                                crate::entities::EntityKind::Item { .. }
                                    | crate::entities::EntityKind::ExperienceOrb { .. }
                            )
                            && (at[0] - pos[0]).abs()
                                < state.entities.body(victim).half_width + 0.5
                            && (at[2] - pos[2]).abs()
                                < state.entities.body(victim).half_width + 0.5
                            && at[1] < pos[1] + 0.8
                            && at[1] + state.entities.body(victim).height > pos[1])
                            .then_some(state.entities.body(victim).id)
                    })
                    .collect();
                for victim in victims {
                    if state
                        .players
                        .get(&victim)
                        .is_some_and(|player| player.creative() || player.spectator())
                    {
                        continue;
                    }
                    if owner_id.is_none_or(|owner| {
                        crate::scoreboard::permits_player_damage(state, owner, victim)
                    }) {
                        damage(
                            state,
                            intents,
                            owner_id.unwrap_or(id),
                            victim,
                            6.0,
                            DamageKind::Magic,
                        );
                    }
                }
            }
            life = life.saturating_sub(1);
            if life == 0 {
                remove(state, id);
                return;
            }
        }
        if let Some(Retained::Fangs {
            warmup: stored_warmup,
            life: stored_life,
            ..
        }) = state.entities.row_mut(slot).special_mob.as_deref_mut()
        {
            *stored_warmup = warmup;
            *stored_life = life;
        }
        return;
    }
    if kind == GoalKind::VexLife {
        if let Some(&Retained::Vex { life, .. }) = state.entities.row(slot).special_mob.as_deref() {
            if life <= 1 {
                remove(state, id);
            } else if let Some(Retained::Vex { life, .. }) =
                state.entities.row_mut(slot).special_mob.as_deref_mut()
            {
                *life -= 1;
            }
        }
        return;
    }
    // Casting recovery and cooldown continue even after the target leaves sight.
    if attack.stage >= 2 {
        if state.tick >= attack.deadline {
            if attack.stage == 2 {
                attack.stage = 3;
                attack.deadline = state.tick + 60;
                metadata(state, slot, 17, EntityDataValue::Byte(0));
            } else {
                attack.stage = 0;
                attack.target = None;
            }
        }
        return;
    }
    let Some((victim, at)) = target(state, slot, targets, 12.0) else {
        attack.stage = 0;
        attack.target = None;
        metadata(state, slot, 17, EntityDataValue::Byte(0));
        return;
    };
    if attack.stage == 0 {
        attack.target = Some(victim);
        attack.stage = 1;
        attack.deadline = state.tick + 20;
        metadata(state, slot, 17, EntityDataValue::Byte(2));
    } else if attack.stage == 1 {
        if attack.target != Some(victim) {
            attack.stage = 0;
            attack.target = None;
            metadata(state, slot, 17, EntityDataValue::Byte(0));
            return;
        }
        if state.tick < attack.deadline {
            return;
        }
        cast_fangs(state, id, at);
        if state.tick >= attack.summon_at {
            summon_vex(state, id);
            attack.summon_at = state.tick + 340;
        }
        attack.stage = 2;
        attack.deadline = state.tick + 20;
    }
}
pub(crate) fn pattern(pos: [f64; 3], target: [f64; 3]) -> Vec<([f64; 3], i32, f32)> {
    let angle = mth::atan2(target[2] - pos[2], target[0] - pos[0]);
    let y = pos[1].min(target[1]);
    let fang = |reach: f64, angle: f64, warmup| {
        (
            [
                pos[0] + f64::from(mth::cos(angle)) * reach,
                y,
                pos[2] + f64::from(mth::sin(angle)) * reach,
            ],
            warmup,
            angle.to_degrees() as f32,
        )
    };
    if distance_sq(pos, target) < 9.0 {
        (0..5)
            .map(|i| fang(1.5, angle + f64::from(i) * std::f64::consts::TAU / 5.0, 0))
            .chain((0..8).map(|i| {
                fang(
                    2.5,
                    angle + f64::from(i) * std::f64::consts::TAU / 8.0 + 1.256_637_1,
                    3,
                )
            }))
            .collect()
    } else {
        (0..16)
            .map(|i| fang(1.25 * f64::from(i + 1), angle, i))
            .collect()
    }
}
fn cast_fangs(state: &mut TickState, owner: EntityId, target: [f64; 3]) {
    let Some(owner_slot) = state.entities.slot_of(owner) else {
        return;
    };
    let uuid = state.entities.row(owner_slot).uuid;
    let pos = state.entities.pos(owner_slot);
    let Some(type_id) = type_id(state, "minecraft:evoker_fangs") else {
        return;
    };
    for (mut at, warmup, yaw) in pattern(pos, target) {
        // Fangs emerge from supported terrain, never through floors or unloaded
        // cells. Match the caster/target vertical band, with one-block descent.
        let x = at[0].floor() as i32;
        let z = at[2].floor() as i32;
        let top = pos[1].max(target[1]).floor() as i32 + 1;
        let bottom = pos[1].min(target[1]).floor() as i32 - 1;
        let floor = (bottom..=top).rev().find(|y| {
            state.world.block_at(x, *y - 1, z).is_some_and(|block| {
                state.registry.materials.classify(block) == mc_physics::BlockMaterial::Solid
            }) && state.world.block_at(x, *y, z).is_some_and(|block| {
                state.registry.materials.classify(block) != mc_physics::BlockMaterial::Solid
            })
        });
        let Some(floor) = floor else { continue };
        at[1] = f64::from(floor);
        let id = state.spawn_mob(type_id, at, 1.0);
        let slot = state.entities.slot_of(id).expect("fang");
        state.entities.set_rot(slot, yaw, 0.0);
        state.entities.row_mut(slot).special_mob = Some(Box::new(Retained::Fangs {
            owner: Some(uuid),
            warmup,
            life: 22,
        }));
        state.hostile.spawned.push(id);
    }
}
fn summon_vex(state: &mut TickState, owner: EntityId) {
    let Some(slot) = state.entities.slot_of(owner) else {
        return;
    };
    let uuid = state.entities.row(slot).uuid;
    let pos = state.entities.pos(slot);
    let count = state.entities.slots().filter(|slot| matches!(state.entities.row(*slot).special_mob.as_deref(), Some(Retained::Vex { owner: Some(value), .. }) if *value == uuid)).count();
    let Some(type_id) = type_id(state, "minecraft:vex") else {
        return;
    };
    for i in 0..3.min(6usize.saturating_sub(count)) {
        let at = [pos[0] + i as f64 - 1.0, pos[1] + 1.0, pos[2] + 1.0];
        let id = state.spawn_mob(type_id, at, 14.0);
        state
            .entities
            .row_mut(state.entities.slot_of(id).expect("vex"))
            .special_mob = Some(Box::new(Retained::Vex {
            owner: Some(uuid),
            life: 1200,
        }));
        state.hostile.spawned.push(id);
    }
}
pub(crate) fn type_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .blueprints
        .iter()
        .find(|(_, blueprint)| blueprint.name == name)
        .map(|(id, _)| *id)
}
pub(crate) fn remove(state: &mut TickState, id: EntityId) {
    if let Some(slot) = state.entities.slot_of(id) {
        state.remove_entity(slot);
        state.hostile.removed.push(id);
    }
}
