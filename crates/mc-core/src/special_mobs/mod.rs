//! Native timed special attacks. Only OLD's retained attack state and spawned
//! auxiliaries survive restart; aiming, beams and burst timers are tick-local.
mod blueprints;
pub(crate) mod persistence;
pub(crate) mod raiders;
mod shooters;
mod summons;
mod wither;

use crate::blueprint::GoalKind;
use crate::boundary::{EntityDataValue, EntityEvent, EntityId};
use crate::effects::{ActiveEffect, EffectId};
use crate::entities::EntitySlot;
use crate::intent::{DamageSource, Footprint, Intent, IntentKind};
use crate::survival::DamageKind;
use crate::tick::TickState;
use mc_physics::mth;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

pub(crate) use blueprints::extend_blueprints;
pub(crate) use wither::{after_blocks, damage_allowed, death_drop, summoned_nearby};

pub(crate) fn homing_velocity(from: [f64; 3], to: [f64; 3]) -> [f64; 3] {
    shooters::velocity(from, to, 0.5)
}

/// Rebuild native metadata after checkpoint/replay; transient attack animations
/// must not resume without their runtime target/timer.
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    crate::copper_golem::restore(state, slot);
    crate::piglin::restore(state, slot);
    let kind = match state.entities.body(slot).kind {
        crate::entities::EntityKind::Mob { type_id } => {
            state.registry.blueprint(type_id).and_then(goal)
        }
        _ => None,
    };
    match kind {
        Some(GoalKind::BlazeBurst) => metadata(state, slot, 16, EntityDataValue::Byte(0)),
        Some(GoalKind::SiegeShot) => metadata(state, slot, 16, EntityDataValue::Boolean(false)),
        Some(GoalKind::Crossbow) => crossbow_appearance(state, slot),
        Some(GoalKind::Alchemist) => {
            metadata(state, slot, 17, EntityDataValue::Boolean(false));
            std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
                None;
        }
        Some(GoalKind::Beam | GoalKind::ElderBeam) => {
            metadata(state, slot, 17, EntityDataValue::VarInt(0))
        }
        Some(GoalKind::Fangs) => metadata(state, slot, 17, EntityDataValue::Byte(0)),
        Some(GoalKind::HomingShot) => metadata(state, slot, 17, EntityDataValue::Byte(0)),
        Some(GoalKind::WitherBarrage) => {
            if let Some(&Retained::Wither { invulnerable }) =
                state.entities.row(slot).special_mob.as_deref()
            {
                metadata(
                    state,
                    slot,
                    19,
                    EntityDataValue::VarInt(invulnerable as i32),
                );
            }
            for index in 16..19 {
                metadata(state, slot, index, EntityDataValue::VarInt(0));
            }
        }
        Some(GoalKind::SonicBoom) => {
            let id = state.entities.body(slot).id;
            state.entities.register_warden(id);
            crate::warden::restore(state, slot);
        }
        _ => {}
    }
}
pub(crate) fn crossbow_appearance(state: &mut TickState, slot: EntitySlot) {
    let phase = match state.entities.row(slot).special_mob.as_deref() {
        Some(Retained::Crossbow(Some(crossbow))) => Some(crossbow.phase),
        _ => None,
    };
    metadata(
        state,
        slot,
        17,
        EntityDataValue::Boolean(phase == Some(CrossbowPhase::Charging)),
    );
    // Idle/restored equipment is authoritative, including authored projectiles.
    if phase.is_none() {
        return;
    }
    let Some(crossbow) = item_id(state, "crossbow") else {
        return;
    };
    let Some(stack) = state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .filter(|stack| stack.item_id == crossbow)
    else {
        return;
    };
    let arrow = (phase == Some(CrossbowPhase::Charged))
        .then(|| item_id(state, "arrow"))
        .flatten();
    let projectiles = &stack.extended().charged_projectiles;
    if match arrow {
        Some(arrow) => projectiles.len() == 1 && projectiles[0].item_id == arrow,
        None => projectiles.is_empty(),
    } {
        return;
    }
    let appearance = std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance);
    let projectiles = &mut appearance.equipment[0]
        .as_mut()
        .expect("held crossbow")
        .extended_mut()
        .charged_projectiles;
    projectiles.clear();
    if let Some(arrow) = arrow {
        projectiles.push(mc_data::ItemStack::new(arrow, 1));
    }
}

fn clear_crossbow_payload(state: &mut TickState, slot: EntitySlot) {
    let crossbow = item_id(state, "crossbow");
    if !state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .is_some_and(|stack| {
            Some(stack.item_id) == crossbow && !stack.extended().charged_projectiles.is_empty()
        })
    {
        return;
    }
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0]
        .as_mut()
        .expect("loaded crossbow")
        .extended_mut()
        .charged_projectiles
        .clear();
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CrossbowPhase {
    Aiming,
    Charging,
    Charged,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Crossbow {
    pub phase: CrossbowPhase,
    pub deadline: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Potion {
    Slowness,
    Poison,
    Harming,
    Healing,
    FireResistance,
    WaterBreathing,
    Swiftness,
    Weakness,
}
impl Potion {
    fn name(self) -> &'static str {
        match self {
            Self::Slowness => "slowness",
            Self::Poison => "poison",
            Self::Harming => "harming",
            Self::Healing => "healing",
            Self::FireResistance => "fire_resistance",
            Self::WaterBreathing => "water_breathing",
            Self::Swiftness => "swiftness",
            Self::Weakness => "weakness",
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum Retained {
    Copper(crate::copper_golem::CopperState),
    Piglin(crate::piglin::Barter),
    Enderman {
        target: Uuid,
        until: u64,
    },
    Crossbow(Option<Crossbow>),
    Witch(Potion),
    Fangs {
        owner: Option<Uuid>,
        warmup: i32,
        life: u32,
    },
    Vex {
        owner: Option<Uuid>,
        life: u32,
    },
    Wither {
        invulnerable: u32,
    },
    /// IronGolem.PlayerCreated: built by a player, never hostile to players.
    PlayerIronGolem,
    Warden(crate::warden::Warden),
}
#[derive(Debug, Clone, Default)]
pub(crate) struct Attack {
    pub stage: i32,
    pub deadline: u64,
    pub target: Option<EntityId>,
    pub drink: Option<Potion>,
    pub summon_at: u64,
}
#[derive(Debug, Default)]
pub(crate) struct Runtime {
    pub attacks: HashMap<EntityId, Attack>,
}

pub(crate) fn goal(blueprint: &crate::blueprint::MobBlueprint) -> Option<GoalKind> {
    blueprint.goals.iter().map(|goal| goal.kind).find(|kind| {
        matches!(
            kind,
            GoalKind::BlazeBurst
                | GoalKind::SiegeShot
                | GoalKind::Crossbow
                | GoalKind::Alchemist
                | GoalKind::WindCharge
                | GoalKind::Beam
                | GoalKind::ElderBeam
                | GoalKind::SonicBoom
                | GoalKind::Fangs
                | GoalKind::HomingShot
                | GoalKind::WitherBarrage
                | GoalKind::FangLife
                | GoalKind::VexLife
        )
    })
}
pub(crate) fn initialize(state: &mut TickState, slot: EntitySlot, type_id: u32) {
    raiders::initialize(state, slot, type_id);
    let Some(kind) = state.registry.blueprint(type_id).and_then(goal) else {
        return;
    };
    let retained = match kind {
        GoalKind::Crossbow => Some(Retained::Crossbow(None)),
        GoalKind::Alchemist => Some(Retained::Witch(Potion::Harming)),
        GoalKind::WitherBarrage => Some(Retained::Wither { invulnerable: 0 }),
        GoalKind::SonicBoom => Some(Retained::Warden(crate::warden::Warden {
            calm_until: state.tick + 1200,
            sniff_at: state.tick + 100,
            decay_at: state.tick + 20,
            ..Default::default()
        })),
        GoalKind::FangLife => Some(Retained::Fangs {
            owner: None,
            warmup: 0,
            life: 22,
        }),
        _ => None,
    };
    state.entities.row_mut(slot).special_mob = retained.map(Box::new);
    if kind == GoalKind::SonicBoom {
        let id = state.entities.body(slot).id;
        state.entities.register_warden(id);
    }
    if matches!(
        kind,
        GoalKind::WitherBarrage | GoalKind::FangLife | GoalKind::VexLife
    ) {
        state.entities.row_mut(slot).persistent = true;
    }
    if kind == GoalKind::Crossbow
        && let Some(item_id) = item_id(state, "crossbow")
    {
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).equipment[0] =
            Some(mc_data::ItemStack::new(item_id, 1));
    }
}
pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    kind: GoalKind,
    targets: &[(EntityId, [f64; 3])],
) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    if state.entities.row(slot).controls.no_ai
        && !matches!(kind, GoalKind::FangLife | GoalKind::VexLife)
    {
        return;
    }
    let mut attack = state.hostile.native.attacks.remove(&id).unwrap_or_default();
    match kind {
        GoalKind::SonicBoom => crate::warden::run(state, intents, id, slot, targets, &mut attack),
        GoalKind::Beam | GoalKind::ElderBeam => {
            beam(state, intents, id, slot, kind, targets, &mut attack)
        }
        GoalKind::Fangs | GoalKind::FangLife | GoalKind::VexLife => {
            summons::run(state, intents, id, kind, targets, &mut attack)
        }
        GoalKind::WitherBarrage => wither::run(state, intents, id, targets, &mut attack),
        _ => shooters::run(state, intents, id, kind, targets, &mut attack),
    }
    if state.entities.slot_of(id).is_some() {
        state.hostile.native.attacks.insert(id, attack);
    }
}
pub(crate) fn beam(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    slot: EntitySlot,
    kind: GoalKind,
    targets: &[(EntityId, [f64; 3])],
    attack: &mut Attack,
) {
    let sonic = kind == GoalKind::SonicBoom;
    let elder = kind == GoalKind::ElderBeam;
    let pos = state.entities.pos(slot);
    if elder && (state.tick + id.0.unsigned_abs() as u64).is_multiple_of(1200) {
        for &(target, at) in targets {
            if distance_sq(pos, at) < 2500.0 {
                effect(
                    state,
                    target,
                    ActiveEffect::new(EffectId::MiningFatigue, 6000, 2),
                );
            }
        }
    }
    let reach = f64::from(state.entities.row(slot).sight_range);
    let valid = |target: &(EntityId, [f64; 3])| {
        if sonic {
            mth::length2(pos[0] - target.1[0], pos[2] - target.1[2]) <= 15.0
                && (pos[1] - target.1[1]).abs() <= 20.0
        } else {
            let d = distance_sq(pos, target.1);
            d <= reach * reach
                && (d > 9.0 || (elder && attack.stage != 0))
                && visible(state, slot, target.1)
        }
    };
    let target = if let Some(target) = attack.target {
        targets
            .iter()
            .find(|(id, _)| *id == target)
            .filter(|target| valid(target))
            .copied()
    } else {
        targets
            .iter()
            .filter(|target| valid(target))
            .min_by(|a, b| distance_sq(pos, a.1).total_cmp(&distance_sq(pos, b.1)))
            .copied()
    };
    // Recovery is not cancelled by a lost target: sonic cannot be spammed by
    // stepping in and out of its range after the hit.
    if sonic && attack.stage >= 2 {
        if state.tick >= attack.deadline {
            if attack.stage == 2 {
                attack.stage = 3;
                attack.deadline = state.tick + 40;
            } else {
                attack.stage = 0;
                attack.target = None;
            }
        }
        return;
    }
    let Some((target, _)) = target else {
        attack.stage = 0;
        attack.target = None;
        if !sonic {
            metadata(state, slot, 17, EntityDataValue::VarInt(0));
        }
        return;
    };
    if attack.stage == 0 {
        attack.target = Some(target);
        attack.stage = 1;
        attack.deadline = state.tick + if sonic { 34 } else { 10 };
        if sonic {
            event(state, id, 62);
        }
    } else if state.tick >= attack.deadline {
        if !sonic && attack.stage == 1 {
            attack.stage = 2;
            attack.deadline = state.tick + if elder { 60 } else { 80 };
            metadata(state, slot, 17, EntityDataValue::VarInt(target.0));
            event(state, id, 21);
        } else {
            let amount = if sonic {
                10.0
            } else {
                state.entities.row(slot).attack_damage + if elder { 3.0 } else { 1.0 }
            };
            damage(state, intents, id, target, amount, DamageKind::Magic);
            if sonic {
                attack.stage = 2;
                attack.deadline = state.tick + 26;
            } else {
                attack.stage = 0;
                attack.target = None;
                metadata(state, slot, 17, EntityDataValue::VarInt(0));
            }
        }
    }
}
pub(crate) fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| (a[i] - b[i]).powi(2)).sum()
}
pub(crate) fn visible(state: &TickState, slot: EntitySlot, target: [f64; 3]) -> bool {
    let mut eye = state.entities.pos(slot);
    eye[1] += state.entities.body(slot).height * 0.85;
    crate::hostile::sight_clear(state, eye, [target[0], target[1] + 1.62, target[2]])
}
pub(crate) fn target(
    state: &TickState,
    slot: EntitySlot,
    targets: &[(EntityId, [f64; 3])],
    range: f64,
) -> Option<(EntityId, [f64; 3])> {
    let pos = state.entities.pos(slot);
    targets
        .iter()
        .filter(|(_, at)| distance_sq(pos, *at) <= range * range && visible(state, slot, *at))
        .min_by(|a, b| distance_sq(pos, a.1).total_cmp(&distance_sq(pos, b.1)))
        .copied()
}
/// [`target`] over a per-tick [`crate::target_grid::TargetGrid`]: same
/// nearest visible target (lowest table index among equal distances).
pub(crate) fn target_in(
    state: &TickState,
    slot: EntitySlot,
    targets: &crate::target_grid::TargetGrid,
    range: f64,
) -> Option<(EntityId, [f64; 3])> {
    let pos = state.entities.pos(slot);
    targets
        .nearest(pos, range, |(_, at)| {
            let dist = distance_sq(pos, *at);
            (dist <= range * range && visible(state, slot, *at)).then_some(dist)
        })
        .map(|(index, _)| targets.table()[index])
}
pub(crate) fn damage(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    actor: EntityId,
    target: EntityId,
    amount: f32,
    kind: DamageKind,
) {
    let seq = state.next_seq(actor);
    let direct_type_id = state
        .entities
        .slot_of(actor)
        .and_then(|slot| match state.entities.body(slot).kind {
            crate::entities::EntityKind::Mob { type_id } => Some(type_id),
            _ => None,
        })
        .unwrap_or(0);
    intents.push(Intent {
        actor,
        seq,
        kind: IntentKind::DamageEntity {
            target,
            amount: state.difficulty.scale_mob_damage(amount),
            kind,
            source: Some(DamageSource::Projectile {
                projectile: actor,
                direct_type_id,
                owner: actor,
                velocity: [0.0; 2],
                punch: 0,
                flame: false,
            }),
        },
        footprint: Footprint::default(),
    });
}
pub(crate) fn effect(state: &mut TickState, target: EntityId, value: ActiveEffect) {
    if let Some(player) = state.players.get_mut(&target) {
        let changes = player.give_effect(value);
        if let Some(conn) = state.connections.entity_connection(target) {
            for change in changes {
                state.push_player(conn, change);
            }
        }
    } else if let Some(slot) = state.entities.slot_of(target) {
        state.entities.row_mut(slot).active_effects.add(value);
        // Duration/amplifier changes can leave the projected flags unchanged;
        // the authoritative effect still needs a durable entity post-image.
        state.appearance_changed.push(target);
    }
    crate::effects::sync_appearance(state, target);
}
pub(crate) fn metadata(state: &mut TickState, slot: EntitySlot, index: u8, value: EntityDataValue) {
    let row = state.entities.row_mut(slot);
    if row
        .appearance
        .extra
        .iter()
        .any(|(at, old)| *at == index && *old == value)
    {
        return;
    }
    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
    if let Some((_, old)) = appearance.extra.iter_mut().find(|(at, _)| *at == index) {
        *old = value;
    } else {
        appearance.extra.push((index, value));
    }
}
pub(crate) fn event(state: &mut TickState, entity: EntityId, event: i8) {
    state
        .hostile
        .events
        .push(EntityEvent::Status { entity, event });
}
pub(crate) fn item_id(state: &TickState, name: &str) -> Option<u32> {
    state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).ok()?)
}

#[cfg(test)]
#[path = "../special_mobs_tests.rs"]
mod tests;
