use super::*;
use crate::projectiles::{ProjectileData, ProjectileKind, ProjectileState};

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
    if kind == GoalKind::Alchemist && drink(state, id, slot, attack, targets) {
        return;
    }
    if kind == GoalKind::WindCharge && attack.stage >= 2 {
        if state.tick >= attack.deadline {
            if attack.stage == 2 {
                if state.entities.ground(slot)
                    && let Some((_, at)) = targets.iter().find(|(id, _)| Some(*id) == attack.target)
                {
                    // Swept walker physics resolves the recovery jump next tick.
                    let pos = state.entities.pos(slot);
                    let dx = at[0] - pos[0];
                    let dz = at[2] - pos[2];
                    let length = mth::length2(dx, dz).max(1.0);
                    state
                        .entities
                        .set_vel(slot, [dx / length * 12.0, 14.0, dz / length * 12.0]);
                    state
                        .entities
                        .set_horizontal_impulse(slot, [dx / length * 12.0, dz / length * 12.0]);
                    state.entities.set_ground(slot, false);
                }
                attack.stage = 3;
                attack.deadline = state.tick + 10;
            } else {
                attack.stage = 0;
                attack.target = None;
            }
        }
        return;
    }
    let range = match kind {
        GoalKind::SiegeShot => 64.0,
        GoalKind::Crossbow => 8.0,
        GoalKind::Alchemist => 10.0,
        GoalKind::WindCharge => 16.0,
        GoalKind::HomingShot => 20.0,
        _ => f64::from(state.entities.row(slot).sight_range),
    };
    let found = target(state, slot, targets, range);
    if kind == GoalKind::HomingShot {
        metadata(
            state,
            slot,
            17,
            EntityDataValue::Byte(if found.is_some() { 100 } else { 0 }),
        );
    }
    let Some((target_id, at)) = found else {
        attack.stage = 0;
        attack.deadline = 0;
        attack.target = None;
        if kind == GoalKind::BlazeBurst {
            metadata(state, slot, 16, EntityDataValue::Byte(0));
        }
        if kind == GoalKind::SiegeShot {
            metadata(state, slot, 16, EntityDataValue::Boolean(false));
        }
        if kind == GoalKind::Crossbow
            && state.entities.row(slot).special_mob.as_deref() != Some(&Retained::Crossbow(None))
        {
            clear_crossbow_payload(state, slot);
            state
                .entities
                .row_mut(slot)
                .set_special_mob(Retained::Crossbow(None));
            crossbow_appearance(state, slot);
        }
        return;
    };
    let pos = state.entities.pos(slot);
    let dist = distance_sq(pos, at).sqrt();
    match kind {
        GoalKind::Crossbow => crossbow(state, id, slot, at),
        GoalKind::BlazeBurst => {
            if state.tick < attack.deadline {
                return;
            }
            if dist < 2.0 {
                damage(
                    state,
                    intents,
                    id,
                    target_id,
                    state.entities.row(slot).attack_damage,
                    DamageKind::Melee,
                );
                attack.deadline = state.tick + 20;
            } else {
                match attack.stage {
                    0 => {
                        attack.stage = 1;
                        attack.deadline = state.tick + 60;
                        metadata(state, slot, 16, EntityDataValue::Byte(1));
                    }
                    1..=3 => {
                        let from = [pos[0], pos[1] + 1.4, pos[2]];
                        if shoot(
                            state,
                            id,
                            ProjectileKind::SmallFireball,
                            from,
                            [at[0], at[1] + 0.9, at[2]],
                            0.1,
                            None,
                        )
                        .is_some()
                        {
                            attack.stage += 1;
                            attack.deadline = state.tick + 6;
                        }
                    }
                    _ => {
                        attack.stage = 0;
                        attack.deadline = state.tick + 100;
                        metadata(state, slot, 16, EntityDataValue::Byte(0));
                    }
                }
            }
        }
        GoalKind::SiegeShot => {
            attack.stage += 1;
            metadata(state, slot, 16, EntityDataValue::Boolean(attack.stage > 10));
            if attack.stage == 20 {
                let horizontal = mth::length2(at[0] - pos[0], at[2] - pos[2]);
                if horizontal > 1e-9 {
                    let from = [
                        pos[0] + (at[0] - pos[0]) / horizontal * 4.0,
                        pos[1] + 2.5,
                        pos[2] + (at[2] - pos[2]) / horizontal * 4.0,
                    ];
                    if shoot(
                        state,
                        id,
                        ProjectileKind::Fireball,
                        from,
                        [at[0], at[1] + 0.9, at[2]],
                        0.1,
                        None,
                    )
                    .is_some()
                    {
                        attack.stage = -40;
                    } else {
                        attack.stage = 19;
                    }
                } else {
                    attack.stage = 19;
                }
            }
        }
        GoalKind::WindCharge => {
            if attack.stage == 0 {
                attack.stage = 1;
                attack.deadline = state.tick + 15;
                attack.target = Some(target_id);
            } else if state.tick >= attack.deadline {
                if attack.target != Some(target_id) {
                    attack.stage = 0;
                    attack.target = None;
                    return;
                }
                let from = [
                    pos[0],
                    pos[1] + state.entities.body(slot).height * 0.5 + 0.3,
                    pos[2],
                ];
                if shoot(
                    state,
                    id,
                    ProjectileKind::BreezeWindCharge,
                    from,
                    [at[0], at[1] + 0.54, at[2]],
                    0.7,
                    None,
                )
                .is_some()
                {
                    attack.stage = 2;
                    attack.deadline = state.tick + 4;
                }
            }
        }
        GoalKind::Alchemist => {
            if attack.target != Some(target_id) {
                attack.target = Some(target_id);
                attack.deadline = state.tick + 60;
            }
            if state.tick < attack.deadline {
                return;
            }
            let target_slot = state.entities.slot_of(target_id).expect("live target");
            let target_row = state.entities.row(target_slot);
            let effects = state
                .players
                .get(&target_id)
                .map_or(&target_row.active_effects, |player| &player.active_effects);
            let health = state
                .players
                .get(&target_id)
                .map_or(target_row.health, |player| player.vitals.health);
            let horizontal = mth::length2(at[0] - pos[0], at[2] - pos[2]);
            let potion = if horizontal >= 8.0 && !effects.has(EffectId::Slowness) {
                Potion::Slowness
            } else if health >= 8.0 && !effects.has(EffectId::Poison) {
                Potion::Poison
            } else if horizontal <= 3.0
                && !effects.has(EffectId::Weakness)
                && crate::natural_spawn::Rng(state.tick ^ id.0 as u64).below(4) == 0
            {
                Potion::Weakness
            } else {
                Potion::Harming
            };
            let from = [pos[0], pos[1] + 1.1, pos[2]];
            let aim = [at[0], at[1] + 1.62 + horizontal * 0.2, at[2]];
            if shoot(
                state,
                id,
                ProjectileKind::Potion,
                from,
                aim,
                if horizontal <= 2.0 { 0.45 } else { 0.75 },
                Some(potion),
            )
            .is_some()
            {
                state
                    .entities
                    .row_mut(slot)
                    .set_special_mob(Retained::Witch(potion));
                attack.deadline = state.tick + 60;
            }
        }
        GoalKind::HomingShot => {
            if attack.deadline == 0 {
                attack.deadline = state.tick + 20;
            } else if state.tick >= attack.deadline {
                let from = [pos[0], pos[1] + 0.5, pos[2]];
                if let Some(bullet) = shoot(
                    state,
                    id,
                    ProjectileKind::ShulkerBullet,
                    from,
                    [at[0], at[1] + 0.9, at[2]],
                    0.15,
                    None,
                ) {
                    let uuid = state
                        .entities
                        .row(state.entities.slot_of(target_id).expect("target"))
                        .uuid;
                    let bullet_slot = state.entities.slot_of(bullet).expect("bullet");
                    state
                        .entities
                        .row_mut(bullet_slot)
                        .projectile
                        .as_mut()
                        .expect("record")
                        .data = ProjectileData::Homing { target: uuid };
                    attack.deadline = state.tick
                        + 20
                        + ((id.0.unsigned_abs() as u64).wrapping_mul(0xD6E8_FEB8_6659_FD93)
                            ^ state.tick.rotate_left(11))
                            % 10
                            * 10;
                }
            }
        }
        _ => {}
    }
}
fn crossbow(state: &mut TickState, id: EntityId, slot: EntitySlot, at: [f64; 3]) {
    let current = match state.entities.row(slot).special_mob.as_deref() {
        Some(&Retained::Crossbow(value)) => value,
        _ => None,
    };
    let next = match current {
        None => Some(Crossbow {
            phase: CrossbowPhase::Aiming,
            deadline: state.tick + 4,
        }),
        Some(value) if state.tick < value.deadline => return,
        Some(value) => match value.phase {
            CrossbowPhase::Aiming => Some(Crossbow {
                phase: CrossbowPhase::Charging,
                deadline: state.tick + 25,
            }),
            CrossbowPhase::Charging => {
                let delay = 20
                    + ((id.0.unsigned_abs() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                        ^ state.tick.rotate_left(17))
                        % 20;
                Some(Crossbow {
                    phase: CrossbowPhase::Charged,
                    deadline: state.tick + delay,
                })
            }
            CrossbowPhase::Charged => {
                let pos = state.entities.pos(slot);
                let from = [pos[0], pos[1] + 1.5, pos[2]];
                let horizontal = mth::length2(at[0] - pos[0], at[2] - pos[2]);
                let velocity = velocity(from, [at[0], at[1] + 0.6 + horizontal * 0.2, at[2]], 1.6);
                if crate::hostile::launch_arrow(state, id, from, velocity, 2.0).is_none() {
                    return;
                }
                clear_crossbow_payload(state, slot);
                None
            }
        },
    };
    state
        .entities
        .row_mut(slot)
        .set_special_mob(Retained::Crossbow(next));
    crossbow_appearance(state, slot);
}
pub(crate) fn velocity(from: [f64; 3], at: [f64; 3], speed: f64) -> [f64; 3] {
    let length = distance_sq(from, at).sqrt().max(1e-9);
    std::array::from_fn(|i| (at[i] - from[i]) / length * speed)
}
pub(crate) fn shoot(
    state: &mut TickState,
    owner: EntityId,
    kind: ProjectileKind,
    from: [f64; 3],
    at: [f64; 3],
    speed: f64,
    potion: Option<Potion>,
) -> Option<EntityId> {
    let slot = state.entities.slot_of(owner)?;
    let mut stack = mc_data::ItemStack::EMPTY;
    if let Some(potion) = potion {
        stack = mc_data::ItemStack::new(item_id(state, "splash_potion")?, 1);
        stack.extended_mut().potion =
            Some(mc_data::Identifier::parse(format!("minecraft:{}", potion.name())).ok()?);
    }
    let mut record = ProjectileState::new(kind, Some(state.entities.row(slot).uuid), stack);
    record.mob_damage = true;
    crate::projectiles::launch(state, record, from, velocity(from, at, speed))
}
fn drink(
    state: &mut TickState,
    id: EntityId,
    slot: EntitySlot,
    attack: &mut Attack,
    targets: &[(EntityId, [f64; 3])],
) -> bool {
    if let Some(potion) = attack.drink {
        if state.tick >= attack.deadline {
            if potion == Potion::Healing {
                let row = state.entities.row_mut(slot);
                row.health = (row.health + 4.0).min(26.0);
                std::sync::Arc::make_mut(&mut row.appearance).health = row.health;
                state.appearance_changed.push(id);
            } else {
                for value in crate::effects::potion_effects(potion.name()) {
                    effect(state, id, value);
                }
            }
            attack.drink = None;
            attack.target = None;
            attack.deadline = 0;
            crate::entity_loot::restore(state, slot);
            metadata(state, slot, 17, EntityDataValue::Boolean(false));
            std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
                None;
        }
        return true;
    }
    let row = state.entities.row(slot);
    let pos = state.entities.pos(slot);
    let eye_water = state
        .world
        .block_at(
            pos[0].floor() as i32,
            (pos[1] + state.entities.body(slot).height * 0.85).floor() as i32,
            pos[2].floor() as i32,
        )
        .and_then(|block| state.registry.blocks.by_id(mc_world::BlockStateId(block)))
        .is_some_and(|block| block.block.id.path() == "water");
    let potion = if eye_water && !row.active_effects.has(EffectId::WaterBreathing) {
        Some(Potion::WaterBreathing)
    } else if state.entities.body(slot).burn_ticks > 0
        && !row.active_effects.has(EffectId::FireResistance)
    {
        Some(Potion::FireResistance)
    } else if row.health < 26.0 {
        Some(Potion::Healing)
    } else if !row.active_effects.has(EffectId::Speed)
        && targets
            .iter()
            .any(|(_, at)| distance_sq(pos, *at) > 121.0 && distance_sq(pos, *at) < 256.0)
    {
        Some(Potion::Swiftness)
    } else {
        None
    };
    if let Some(potion) = potion {
        attack.drink = Some(potion);
        attack.deadline = state.tick + 32;
        state
            .entities
            .row_mut(slot)
            .set_special_mob(Retained::Witch(potion));
        state.entities.row_mut(slot).speed *= 0.25;
        metadata(state, slot, 17, EntityDataValue::Boolean(true));
        if let Some(item_id) = item_id(state, "potion") {
            let mut stack = mc_data::ItemStack::new(item_id, 1);
            stack.extended_mut().potion = Some(
                mc_data::Identifier::parse(format!("minecraft:{}", potion.name())).expect("potion"),
            );
            std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
                Some(stack);
        }
        true
    } else {
        false
    }
}
