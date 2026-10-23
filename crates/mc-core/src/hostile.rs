//! Skeleton shooting and swelling creepers (ADR 0013, Mob blueprints).
//!
//! Ranged mobs launch into the shared retained projectile authority. Creeper
//! fuses remain transient; projectile flight, ammunition and impacts are durable.

use mc_physics::mth;
use std::collections::HashMap;
use std::sync::Arc;

use mc_physics::BlockMaterial;

use crate::blueprint::{GoalKind, MobBlueprint};
use crate::boundary::{EntityDataValue, EntityId, ExplosionEvent};
use crate::entities::{EntityKind, EntitySlot};
use crate::intent::{Footprint, Intent, IntentKind};
use crate::natural_spawn::Rng;
use crate::survival::Difficulty;
use crate::target_grid::TargetGrid;
use crate::tick::TickState;

/// A skeleton backs off from a target closer than this (vanilla
/// `RangedBowAttackGoal` strafes back inside a quarter of its squared radius).
pub(crate) const BOW_BACKOFF: f64 = 7.5;
/// Back-off walks at half speed.
pub(crate) const BACKOFF_SPEED: f64 = 0.5;
/// Creeper fuse length in ticks (vanilla `maxSwell`).
pub const FUSE_TICKS: u32 = 30;
/// A lit fuse burns down while the target is farther than this.
const FUSE_KEEP_DISTANCE: f64 = 7.0;
/// Vanilla creeper blast power (`explosionRadius` 3, uncharged).
pub const CREEPER_POWER: f32 = 3.0;

/// Arrow launch speed in blocks per tick (vanilla skeleton `shoot` 1.6).
const ARROW_SPEED: f64 = 1.6;
/// Vanilla `RangedBowAttackGoal` draws for twenty ticks, then waits forty
/// ticks after release (twenty on hard) before starting the next draw.
const BOW_DRAW_TICKS: u32 = 20;
const BOW_INTERVAL: u32 = 40;
const HARD_BOW_INTERVAL: u32 = 20;
const BOW_UNSEEN_TIMEOUT: u32 = 60;
/// Eye height of a player, the point a skeleton must see.
const PLAYER_EYE: f64 = 1.62;
/// `Creeper.DATA_SWELL_DIR` (-1 idle, 1 swelling): the client animates the
/// fuse from it. Indices counted on the 26.1.2 server jar: entity 0..7,
/// living 8..14, mob 15, creeper 16..18.
pub const CREEPER_SWELL_DIR_INDEX: u8 = 16;

/// What [`run`] does for a mob; resolved from its blueprint goals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Special {
    Ranged,
    Swell,
    Projectile,
    Native(GoalKind),
}

pub(crate) fn special_of(blueprint: &MobBlueprint) -> Option<Special> {
    if blueprint.has_goal(GoalKind::RangedAttack) {
        Some(Special::Ranged)
    } else if blueprint.has_goal(GoalKind::Swell) {
        Some(Special::Swell)
    } else if blueprint.has_goal(GoalKind::Projectile) {
        Some(Special::Projectile)
    } else {
        crate::special_mobs::goal(blueprint).map(Special::Native)
    }
}

#[derive(Debug, Default)]
struct BowDraw {
    ticks: u32,
    unseen: u32,
}

/// Runtime state of the hostile systems plus what they stage for the
/// tick's change set.
#[derive(Debug, Default)]
pub struct HostileState {
    fuses: HashMap<EntityId, u32>,
    bows: HashMap<EntityId, BowDraw>,
    /// Goal-based chase memory only; rebuilt from live rows each tick, never
    /// persisted or used as a wake subscription.
    pub(crate) targets: rustc_hash::FxHashMap<EntityId, (crate::target_grid::Target, f64)>,
    pub(crate) native: crate::special_mobs::Runtime,
    pub(crate) events: Vec<crate::boundary::EntityEvent>,
    /// Blasts committed this tick (drained into the change set).
    pub(crate) explosions: Vec<ExplosionEvent>,
    /// Item entities blasts spawned (drained into the change set).
    pub(crate) spawned: Vec<EntityId>,
    /// Journaled mobs [`run`] removed (exploded creepers).
    pub(crate) removed: Vec<EntityId>,
}

/// Native special brains and enderman anger own their target policy already.
pub(crate) fn retains_target(blueprint: &MobBlueprint) -> bool {
    !matches!(special_of(blueprint), Some(Special::Native(_)))
        && blueprint.name != "minecraft:enderman"
        && (blueprint.has_goal(GoalKind::MeleeChase)
            || blueprint.has_goal(GoalKind::RangedAttack)
            || blueprint.has_goal(GoalKind::Swell))
}

/// Exact one-target validation, including eligibility changes that must not
/// leave creative, disconnected or dimension-transferred players attackable.
pub(crate) fn refresh_targets(state: &mut TickState) {
    let mut targets = std::mem::take(&mut state.hostile.targets);
    targets.retain(|&actor, ((target, at), dist)| {
        let Some(slot) = state.entities.slot_of(actor) else {
            return false;
        };
        let EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
            return false;
        };
        let Some(blueprint) = state.registry.blueprint(type_id) else {
            return false;
        };
        if !retains_target(blueprint)
            || state.entities.row(slot).controls.no_ai
            || (blueprint.capabilities.night_aggro_only
                && !crate::natural_spawn::is_night(state.day_time))
            || (blueprint.category == crate::blueprint::MobCategory::Monster
                && state.difficulty == Difficulty::Peaceful)
        {
            return false;
        }
        let Some(victim) = state.entities.slot_of(*target) else {
            return false;
        };
        if actor == *target || state.entities.row(victim).health <= 0.0 {
            return false;
        }
        if matches!(state.entities.body(victim).kind, EntityKind::Player { .. }) {
            let Some(player) = state.players.get(target) else {
                return false;
            };
            if !admits_target(player)
                || state.session_locked(player.connection)
                || matches!(
                    state.entities.row(slot).special_mob.as_deref(),
                    Some(crate::special_mobs::Retained::PlayerIronGolem)
                )
            {
                return false;
            }
        }
        *at = state.entities.pos(victim);
        *dist = crate::target_grid::distance(state.entities.pos(slot), *at);
        *dist <= f64::from(state.entities.row(slot).sight_range)
    });
    state.hostile.targets = targets;
}

/// The damage commit, not a polling sensor, replaces a goal-based mob's target.
/// Creepers and slimes do not have vanilla HurtByTargetGoal.
pub(crate) fn retaliate(state: &mut TickState, victim: EntitySlot, attacker: EntityId) {
    let body = state.entities.body(victim);
    let EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let Some(blueprint) = state.registry.blueprint(type_id) else {
        return;
    };
    if !retains_target(blueprint)
        || blueprint.has_goal(GoalKind::Swell)
        || matches!(
            blueprint.name.as_str(),
            "minecraft:slime" | "minecraft:magma_cube"
        )
    {
        return;
    }
    let Some(target) = state.entities.slot_of(attacker) else {
        return;
    };
    if attacker != body.id
        && matches!(
            state.entities.body(target).kind,
            EntityKind::Player { .. } | EntityKind::Mob { .. }
        )
    {
        state
            .hostile
            .targets
            .insert(body.id, ((attacker, state.entities.pos(target)), 0.0));
    }
}

impl HostileState {
    /// Ticks a creeper's fuse has burned.
    pub(crate) fn fuse(&self, id: EntityId) -> u32 {
        self.fuses.get(&id).copied().unwrap_or(0)
    }

    /// Whether a special mob with no live survival/adventure target can skip
    /// this tick's serial pass. The caller has already established target
    /// absence (see [`has_live_targets`]).
    ///
    /// Ranged is skippable only without an in-progress bow draw; Swell only
    /// with fuse 0 and neutral swell metadata (`swell_metadata_neutral`).
    /// Every other special never skips, so a positive fuse or retained
    /// swelling metadata still reaches the serial pass to unwind/reset.
    pub(crate) fn idle_without_targets(
        &self,
        id: EntityId,
        special: Special,
        swell_metadata_neutral: bool,
    ) -> bool {
        match special {
            Special::Ranged => !self.bows.contains_key(&id),
            Special::Swell => self.fuse(id) == 0 && swell_metadata_neutral,
            Special::Projectile | Special::Native(_) => false,
        }
    }
}

pub(crate) fn swell_metadata_neutral(row: &crate::entities::ColdRow) -> bool {
    row.appearance
        .extra
        .iter()
        .find(|(index, _)| *index == CREEPER_SWELL_DIR_INDEX)
        .is_none_or(|(_, value)| *value == EntityDataValue::VarInt(-1))
}

/// One serial pass over this tick's active special mobs. NoAI bodies retain
/// restart-local decisions while absent from the active pass.
pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    specials: &[(EntityId, Special)],
) {
    if specials.is_empty() {
        let entities = &state.entities;
        state
            .hostile
            .fuses
            .retain(|id, _| entities.slot_of(*id).is_some());
        state
            .hostile
            .bows
            .retain(|id, _| entities.slot_of(*id).is_some());
        state
            .hostile
            .native
            .attacks
            .retain(|id, _| entities.slot_of(*id).is_some());
        return;
    }
    let targets = player_targets(state);
    let target_grid = TargetGrid::for_consumers(&targets, specials.len());
    let mut gone: Vec<EntityId> = Vec::new();
    for &(id, special) in specials {
        // Targetless no-bow Ranged skips before any slot/ColdRow lookup.
        // Swell never skips here (neutrality needs the row, so `false`
        // defers it to the post-lookup guard); Ranged ignores the flag.
        let targets_empty = targets.is_empty() && !state.hostile.targets.contains_key(&id);
        if special == Special::Projectile
            || targets_empty && state.hostile.idle_without_targets(id, special, false)
        {
            continue;
        }
        let active = state.entities.slot_of_any(id).is_some_and(|slot| {
            if targets_empty
                && state.hostile.idle_without_targets(
                    id,
                    special,
                    special == Special::Swell && swell_metadata_neutral(state.entities.row(slot)),
                )
            {
                return false;
            }
            let pos = state.entities.pos(slot);
            state.world.is_active(
                (pos[0].floor() as i32).div_euclid(16),
                (pos[2].floor() as i32).div_euclid(16),
            )
        });
        if !active {
            continue;
        }
        match special {
            Special::Ranged => shoot(state, id, &target_grid),
            Special::Swell => swell(state, intents, id, &target_grid, &mut gone),
            Special::Projectile => {}
            Special::Native(kind) => crate::special_mobs::run(state, intents, id, kind, &targets),
        }
    }
    // Removal swaps the last row into the hole: highest slots first.
    let mut slots: Vec<EntitySlot> = gone
        .iter()
        .filter_map(|id| state.entities.slot_of_any(*id))
        .collect();
    slots.sort_unstable_by_key(|slot| std::cmp::Reverse(slot.0));
    slots.dedup();
    for slot in slots {
        state.remove_entity(slot);
    }
    let entities = &state.entities;
    state
        .hostile
        .fuses
        .retain(|id, _| entities.slot_of(*id).is_some());
    state
        .hostile
        .bows
        .retain(|id, _| entities.slot_of(*id).is_some());
    state
        .hostile
        .native
        .attacks
        .retain(|id, _| entities.slot_of(*id).is_some());
}

/// Whether a player admits monster targeting: survival/adventure with a live
/// row (not creative, not spectating). Shared by [`has_live_targets`] and
/// `player_targets` so the allocation-free check agrees with the table.
fn admits_target(player: &crate::player::PlayerState) -> bool {
    !player.creative() && !player.spectator()
}

/// Whether any live player can be targeted this tick, without allocating:
/// exactly `!player_targets(state).is_empty()`.
pub(crate) fn has_live_targets(state: &TickState) -> bool {
    state.players.iter().any(|(id, player)| {
        admits_target(player)
            && !state.session_locked(player.connection)
            && state.entities.slot_of(*id).is_some()
    })
}

/// Live players a monster may target: not creative, not spectating.
fn player_targets(state: &TickState) -> Vec<(EntityId, [f64; 3])> {
    let mut out: Vec<(EntityId, [f64; 3])> = state
        .players
        .iter()
        .filter(|(_, player)| admits_target(player) && !state.session_locked(player.connection))
        .filter_map(|(id, _)| {
            state
                .entities
                .slot_of(*id)
                .map(|slot| (*id, state.entities.pos(slot)))
        })
        .collect();
    out.sort_unstable_by_key(|(id, _)| id.0);
    out
}

fn nearest(
    state: &TickState,
    actor: EntityId,
    targets: &TargetGrid<'_>,
    from: [f64; 3],
    reach: f64,
) -> Option<([f64; 3], f64)> {
    if let Some(&((target, _), _)) = state.hostile.targets.get(&actor) {
        let slot = state.entities.slot_of(target)?;
        let at = state.entities.pos(slot);
        let dist = crate::target_grid::distance(from, at);
        return (dist <= reach).then_some((at, dist));
    }
    targets
        .nearest_within(from, reach, |_| true)
        .map(|(index, distance)| (targets.table()[index].1, distance))
}

fn distance(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

/// Whether no solid block lies between `from` and `to` (sampled every
/// quarter block, the end cells excluded; unloaded space blocks). Shared
/// with the explosion exposure rays.
pub(crate) fn sight_clear(state: &TickState, from: [f64; 3], to: [f64; 3]) -> bool {
    let length = distance(from, to);
    let steps = (length / 0.25).ceil().max(1.0) as u32;
    (1..steps).all(|i| {
        let t = f64::from(i) / f64::from(steps);
        let at = |axis: usize| (from[axis] + (to[axis] - from[axis]) * t).floor() as i32;
        state
            .world
            .block_at(at(0), at(1), at(2))
            .is_some_and(|block| state.registry.materials.classify(block) != BlockMaterial::Solid)
    })
}

fn rng_for(state: &TickState, id: EntityId) -> Rng {
    Rng((state.world.seed() as u64)
        ^ state.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (id.0 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9))
}

/// Writes changed runtime metadata only. No-op reads take no mutable custody;
/// the edge diffs real appearance changes, and none is journaled here.
fn set_extra(
    state: &mut TickState,
    slot: EntitySlot,
    index: u8,
    default: EntityDataValue,
    value: EntityDataValue,
) {
    let row = state.entities.row(slot);
    let current = row
        .appearance
        .extra
        .iter()
        .find(|(at, _)| *at == index)
        .map_or(&default, |(_, value)| value);
    if *current == value {
        return;
    }
    let row = state.entities.row_mut(slot);
    let appearance = Arc::make_mut(&mut row.appearance);
    match appearance.extra.iter_mut().find(|(at, _)| *at == index) {
        Some(entry) => entry.1 = value,
        None => appearance.extra.push((index, value)),
    }
}

// --- skeleton ---------------------------------------------------------

/// Shoot the nearest visible player in range once the cooldown ran down
/// (vanilla `AbstractSkeleton.performRangedAttack`: aim at the target's
/// body with a gravity lift, 1.6 blocks/tick, inaccuracy `14 - 4 *
/// difficulty`).
fn shoot(state: &mut TickState, id: EntityId, targets: &TargetGrid<'_>) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    if body.attack_cooldown > 0 {
        return;
    }
    let (reach, height, base_damage) =
        (f64::from(row.attack_reach), body.height, row.attack_damage);
    let pos = state.entities.pos(slot);
    let Some((target, _)) = nearest(state, id, targets, pos, reach) else {
        state.hostile.bows.remove(&id);
        return;
    };
    let eye = [pos[0], pos[1] + height * 0.85, pos[2]];
    let visible = sight_clear(state, eye, [target[0], target[1] + PLAYER_EYE, target[2]]);
    if let Some(draw) = state.hostile.bows.get_mut(&id) {
        draw.ticks = draw.ticks.saturating_add(1);
        draw.unseen = if visible { 0 } else { draw.unseen + 1 };
        if !visible && draw.unseen > BOW_UNSEEN_TIMEOUT {
            state.hostile.bows.remove(&id);
            return;
        }
        if !visible || draw.ticks < BOW_DRAW_TICKS {
            return;
        }
    } else {
        if visible {
            state.hostile.bows.insert(id, BowDraw::default());
        }
        return;
    }
    let muzzle = [eye[0], eye[1] - 0.1, eye[2]];
    let dx = target[0] - muzzle[0];
    let dz = target[2] - muzzle[2];
    // Vanilla aims at the body third of the target and lifts for gravity.
    let dy = target[1] + 0.6 - muzzle[1];
    let lift = mth::length2(dx, dz) * 0.2;
    let aim = [dx, dy + lift, dz];
    let length = (aim[0] * aim[0] + aim[1] * aim[1] + aim[2] * aim[2]).sqrt();
    let inaccuracy = match state.difficulty {
        Difficulty::Peaceful => 14.0,
        Difficulty::Easy => 10.0,
        Difficulty::Normal => 6.0,
        Difficulty::Hard => 2.0,
    };
    let mut rng = rng_for(state, id);
    let spread = 0.017_227_5 * inaccuracy;
    let velocity: [f64; 3] = std::array::from_fn(|axis| {
        (aim[axis] / length + (rng.unit() - rng.unit()) * spread) * ARROW_SPEED
    });
    if launch_arrow(state, id, muzzle, velocity, base_damage).is_some() {
        state.hostile.bows.remove(&id);
        state.entities.body_mut(slot).attack_cooldown = if state.difficulty == Difficulty::Hard {
            HARD_BOW_INTERVAL
        } else {
            BOW_INTERVAL
        };
    }
}

/// Spawn an arrow at `from` flying `velocity` blocks per tick; it hits for
/// `ceil(speed * base_damage)` (vanilla) and never hits `owner`.
pub(crate) fn launch_arrow(
    state: &mut TickState,
    owner: EntityId,
    from: [f64; 3],
    velocity: [f64; 3],
    base_damage: f32,
) -> Option<EntityId> {
    let owner_uuid = state
        .entities
        .slot_of(owner)
        .map(|slot| state.entities.row(slot).uuid);
    let mut record = crate::projectiles::ProjectileState::new(
        crate::projectiles::ProjectileKind::Arrow,
        owner_uuid,
        mc_data::ItemStack::EMPTY,
    );
    record.damage = base_damage;
    record.mob_damage = true;
    if state.entities.slot_of(owner).is_some_and(|slot| {
        matches!(state.entities.body(slot).kind, crate::entities::EntityKind::Mob { type_id }
            if state.registry.blueprint(type_id).is_some_and(|bp| bp.name == "minecraft:bogged"))
    }) {
        let item = mc_data::Identifier::parse("minecraft:tipped_arrow").expect("vanilla item");
        let item_id = state.registry.items.id_of(&item)?;
        record.stack = mc_data::ItemStack::new(item_id, 1);
        record
            .stack
            .extended_mut()
            .potion_custom_effects
            .push(mc_data::item_stack::PotionEffect {
                id: mc_data::Identifier::parse("minecraft:poison").expect("vanilla effect"),
                amplifier: 0,
                duration: 100,
                ambient: false,
                show_particles: true,
                show_icon: true,
                hidden: None,
            });
    }
    crate::projectiles::launch(state, record, from, velocity)
}

// --- swell (creeper) ----------------------------------------------------

/// Clients animate the fuse from [`CREEPER_SWELL_DIR_INDEX`] only on the
/// creeper wire layout; other presentations keep the fuse server-side.
static CREEPER_WIRE_TYPE: std::sync::LazyLock<Option<u32>> =
    std::sync::LazyLock::new(|| crate::piglin::type_id_of("minecraft:creeper"));

/// Advance a swell fuse (vanilla `SwellGoal`): it starts within the
/// blueprint's reach of a visible player, keeps burning while the player
/// stays within 7 blocks in sight and loses a tick per tick otherwise. The
/// fuse length and blast are the blueprint's [`Capabilities::explodes`](crate::blueprint::Capabilities::explodes).
fn swell(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    targets: &TargetGrid<'_>,
    gone: &mut Vec<EntityId>,
) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let body = state.entities.body(slot);
    let crate::entities::EntityKind::Mob { type_id } = body.kind else {
        return;
    };
    let Some(explosion) = state
        .registry
        .blueprint(type_id)
        .and_then(|blueprint| blueprint.capabilities.explodes)
    else {
        return;
    };
    let row = state.entities.row(slot);
    let (start, height) = (f64::from(row.attack_reach), body.height);
    let pos = state.entities.pos(slot);
    let fuse = state.hostile.fuse(id);
    let eye = [pos[0], pos[1] + height * 0.85, pos[2]];
    let swelling =
        nearest(state, id, targets, pos, FUSE_KEEP_DISTANCE).is_some_and(|(target, dist)| {
            dist <= FUSE_KEEP_DISTANCE
                && (fuse > 0 || dist < start)
                && sight_clear(state, eye, [target[0], target[1] + PLAYER_EYE, target[2]])
        });
    let next = if swelling {
        fuse + 1
    } else {
        fuse.saturating_sub(1)
    };
    if Some(crate::blueprint::wire_type_id(type_id)) == *CREEPER_WIRE_TYPE {
        set_extra(
            state,
            slot,
            CREEPER_SWELL_DIR_INDEX,
            EntityDataValue::VarInt(-1),
            EntityDataValue::VarInt(if swelling { 1 } else { -1 }),
        );
    }
    if next >= explosion.fuse_ticks {
        let seq = state.next_seq(id);
        intents.push(Intent {
            actor: id,
            seq,
            kind: IntentKind::Explode {
                center: pos,
                power: explosion.power,
                mob_grief: Some(if explosion.griefing {
                    crate::command_rules::MobGrief::Creeper
                } else {
                    crate::command_rules::MobGrief::Never
                }),
                kill_credit: crate::statistics::living_type(state, id).map(|type_id| (id, type_id)),
            },
            footprint: Footprint::default(),
        });
        state.hostile.fuses.remove(&id);
        state.hostile.removed.push(id);
        gone.push(id);
    } else if next == 0 {
        state.hostile.fuses.remove(&id);
    } else {
        state.hostile.fuses.insert(id, next);
    }
}
