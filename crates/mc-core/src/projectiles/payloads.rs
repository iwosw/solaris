//! Native projectile payloads. Flight owns collision; this module owns retained
//! impact effects and area-payload timers, without a second simulation clock.
use mc_physics::mth;
use std::collections::BTreeMap;
use std::sync::Arc;

use mc_data::ItemStack;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
    ProjectileData, ProjectileKind, ProjectileState, damage, launch, owner_id, remove, touch,
};
use crate::boundary::{BlockPos, EntityDataValue, EntityId, ExplosionEvent, PlayerChange};
use crate::effects::{ActiveEffect, EffectId};
use crate::entities::EntityKind;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::survival::DamageKind;
use crate::tick::TickState;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CloudState {
    pub radius: f32,
    pub waiting: bool,
    pub dragon: bool,
    pub duration: u32,
    pub wait_time: u32,
    pub reapplication_delay: u32,
    pub radius_on_use: f32,
    pub radius_per_tick: f32,
    pub next_tick: u64,
    pub expires_tick: u64,
    /// Persistent identity, never a session or dense slot that changes on replay.
    pub affected_until: BTreeMap<Uuid, u64>,
}

impl CloudState {
    /// AreaEffectCloud constructor defaults from the local 26.1.2 bytecode.
    pub fn empty(tick: u64) -> Self {
        Self {
            radius: 3.0,
            waiting: false,
            dragon: false,
            duration: u32::MAX,
            wait_time: 20,
            reapplication_delay: 20,
            radius_on_use: 0.0,
            radius_per_tick: 0.0,
            next_tick: tick.saturating_add(20),
            expires_tick: u64::MAX,
            affected_until: BTreeMap::new(),
        }
    }

    pub fn lingering(tick: u64) -> Self {
        Self {
            radius: 3.0,
            waiting: true,
            dragon: false,
            duration: 600,
            wait_time: 10,
            reapplication_delay: 100,
            radius_on_use: -0.5,
            // OLD subtracts 0.025 on each five-tick pulse.
            radius_per_tick: -0.005,
            next_tick: tick.saturating_add(10),
            expires_tick: tick.saturating_add(610),
            affected_until: BTreeMap::new(),
        }
    }

    pub fn dragon(tick: u64) -> Self {
        Self {
            radius: 3.0,
            waiting: false,
            dragon: true,
            duration: 600,
            wait_time: 0,
            reapplication_delay: 20,
            radius_on_use: 0.0,
            radius_per_tick: 4.0 / 600.0,
            next_tick: tick.saturating_add(5),
            expires_tick: tick.saturating_add(600),
            affected_until: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FireworkState {
    pub lifetime: u32,
}

fn random(seed: u64) -> u64 {
    let mut value = seed.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn distance_sq(a: [f64; 3], b: [f64; 3]) -> f64 {
    a.into_iter().zip(b).map(|(a, b)| (a - b).powi(2)).sum()
}

fn entity_name(state: &TickState, id: EntityId) -> Option<&str> {
    let slot = state.entities.slot_of(id)?;
    match state.entities.body(slot).kind {
        EntityKind::Player { .. } => Some("minecraft:player"),
        EntityKind::Mob { type_id } => state.registry.blueprint(type_id).map(|bp| bp.name.as_str()),
        _ => None,
    }
}

fn living(state: &TickState, id: EntityId) -> bool {
    let Some(slot) = state.entities.slot_of(id) else {
        return false;
    };
    if state.entities.row(slot).health <= 0.0 {
        return false;
    }
    if let Some(player) = state.players.get(&id) {
        return !player.vitals.is_dead()
            && player.game_mode != crate::commands::GameMode::Spectator;
    }
    entity_name(state, id)
        .and_then(mc_data::entity_types::entity_type_contract_26_1_2_by_name)
        .is_some_and(|contract| {
            contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
        })
}

fn targets(state: &TickState, position: [f64; 3], radius: f64) -> Vec<EntityId> {
    let mut ids: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let id = state.entities.body(slot).id;
            (living(state, id) && distance_sq(state.entities.pos(slot), position) < radius * radius)
                .then_some(id)
        })
        .collect();
    ids.sort_unstable();
    ids
}

fn lightning_targets(state: &TickState, position: [f64; 3]) -> Vec<EntityId> {
    let mut ids: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            let pos = state.entities.pos(slot);
            ((living(state, body.id)
                || matches!(body.kind, crate::entities::EntityKind::Item { .. }))
                && body.alive
                && (pos[0] - position[0]).abs() <= 3.0
                && (pos[2] - position[2]).abs() <= 3.0
                && pos[1] >= position[1] - 3.0
                && pos[1] <= position[1] + 9.0)
                .then_some(body.id)
        })
        .collect();
    ids.sort_unstable();
    ids
}

fn pvp_permitted(state: &TickState, owner: Option<EntityId>, target: EntityId) -> bool {
    owner.is_none_or(|owner| {
        owner == target
            || !state.players.contains_key(&owner)
            || !state.players.contains_key(&target)
            || crate::command_rules::boolean(state, "pvp")
    })
}

fn potion_effects(stack: &ItemStack) -> Vec<ActiveEffect> {
    stack
        .extended()
        .potion
        .as_ref()
        .filter(|potion| potion.namespace() == "minecraft")
        .map_or_else(Vec::new, |potion| {
            crate::effects::potion_effects(potion.path())
        })
}

fn potion_color(stack: &ItemStack) -> i32 {
    let (mut red, mut green, mut blue, mut weight) = (0_i32, 0_i32, 0_i32, 0_i32);
    for effect in potion_effects(stack) {
        let color = effect.id.color();
        let count = i32::from(effect.amplifier) + 1;
        red += ((color >> 16) & 255) * count;
        green += ((color >> 8) & 255) * count;
        blue += (color & 255) * count;
        weight += count;
    }
    if weight == 0 {
        -13_083_194
    } else {
        (0xff00_0000_u32 as i32)
            | ((red / weight) << 16)
            | ((green / weight) << 8)
            | (blue / weight)
    }
}

/// Instant health/damage swap for [`Capabilities::inverted_healing`](crate::blueprint::Capabilities::inverted_healing).
fn inverted(state: &TickState, target: EntityId) -> bool {
    let Some(slot) = state.entities.slot_of(target) else {
        return false;
    };
    match state.entities.body(slot).kind {
        EntityKind::Mob { type_id } => state
            .registry
            .blueprint(type_id)
            .is_some_and(|blueprint| blueprint.capabilities.inverted_healing),
        _ => false,
    }
}

pub(crate) fn give_effect(state: &mut TickState, target: EntityId, effect: ActiveEffect) {
    if let Some(player) = state.players.get_mut(&target) {
        let connection = player.connection;
        let changes = player.give_effect(effect);
        for change in changes {
            state.push_player(connection, change);
        }
    } else if let Some(slot) = state.entities.slot_of(target) {
        state.entities.row_mut(slot).active_effects.add(effect);
    }
    crate::effects::sync_appearance(state, target);
    touch(state, target);
}

fn heal(state: &mut TickState, target: EntityId, amount: f32) {
    let Some(slot) = state.entities.slot_of(target) else {
        return;
    };
    let max = match state.entities.body(slot).kind {
        EntityKind::Mob { type_id } => state
            .registry
            .blueprint(type_id)
            .map_or(20.0, |bp| bp.params.max_health),
        _ => 20.0,
    };
    let health = if let Some(player) = state.players.get_mut(&target) {
        if player.vitals.is_dead() {
            return;
        }
        player.vitals.health = (player.vitals.health + amount).min(max);
        let health = player.vitals.health;
        let connection = player.connection;
        let change = player.health_change();
        state.push_player(connection, change);
        health
    } else {
        (state.entities.row(slot).health + amount).min(max)
    };
    state.entities.row_mut(slot).health = health;
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).health = health;
    touch(state, target);
}

/// Potion effects are native core effects. Instant effects resolve now rather
/// than being inserted as duration-one effects and running at full strength later.
pub(crate) fn apply_potion(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &ProjectileState,
    target: EntityId,
    scale: f64,
    cloud: bool,
) {
    for mut effect in potion_effects(&record.stack) {
        if !matches!(effect.id, EffectId::InstantHealth | EffectId::InstantDamage) {
            effect.duration = if cloud {
                effect.duration / 4
            } else {
                (f64::from(effect.duration) * scale + 0.5) as i32
            };
            if effect.duration < 20 {
                continue;
            }
        }
        deliver_effect(
            state,
            intents,
            id,
            record,
            target,
            effect,
            if cloud { 0.5 } else { scale },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn deliver_effect(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &ProjectileState,
    target: EntityId,
    effect: ActiveEffect,
    scale: f64,
) {
    if matches!(effect.id, EffectId::InstantHealth | EffectId::InstantDamage) {
        let owner = owner_id(state, record);
        let hurting = (effect.id == EffectId::InstantDamage) != inverted(state, target);
        let base = if hurting { 6.0 } else { 4.0 };
        let amount =
            (scale * base * 2.0_f64.powi(i32::from(effect.amplifier).min(30)) + 0.5).floor() as f32;
        if amount > 0.0 {
            if hurting && pvp_permitted(state, owner, target) {
                damage(
                    state,
                    intents,
                    id,
                    target,
                    amount,
                    DamageKind::Magic,
                    owner,
                    [0.0; 3],
                );
            } else if !hurting {
                heal(state, target, amount);
            }
        }
    } else {
        give_effect(state, target, effect);
    }
}

pub(crate) fn arrow_effects(
    state: &mut TickState,
    _intents: &mut Vec<Intent>,
    id: EntityId,
    record: &ProjectileState,
    target: EntityId,
) {
    if record.kind == ProjectileKind::SpectralArrow {
        stage_effect(
            state,
            id,
            target,
            ActiveEffect::new(EffectId::Glowing, 200, 0),
        );
    } else {
        for mut effect in potion_effects(&record.stack) {
            if !matches!(effect.id, EffectId::InstantHealth | EffectId::InstantDamage) {
                effect.duration = (effect.duration / 8).max(1);
            }
            stage_effect(state, id, target, effect);
        }
        for effect in &record.stack.extended().potion_custom_effects {
            if let Some(effect_id) = EffectId::from_name(effect.id.as_str()) {
                let mut active = ActiveEffect::new(effect_id, effect.duration, effect.amplifier);
                active.ambient = effect.ambient;
                active.show_particles = effect.show_particles;
                active.show_icon = effect.show_icon;
                stage_effect(state, id, target, active);
            }
        }
    }
}

fn cloud_targets(state: &TickState, position: [f64; 3], cloud: &CloudState) -> Vec<EntityId> {
    if !cloud.dragon {
        return targets(state, position, f64::from(cloud.radius));
    }
    let radius_sq = f64::from(cloud.radius).powi(2);
    let mut ids: Vec<_> = state
        .entities
        .slots()
        .filter_map(|slot| {
            let id = state.entities.body(slot).id;
            let at = state.entities.pos(slot);
            (living(state, id)
                && (at[0] - position[0]).powi(2) + (at[2] - position[2]).powi(2) <= radius_sq)
                .then_some(id)
        })
        .collect();
    ids.sort_unstable();
    ids
}

/// OLD firework exposure uses actual collision boxes, including partial blocks,
/// and rejects unavailable resident cells instead of treating them as air.
fn ray_clear(state: &TickState, from: [f64; 3], to: [f64; 3]) -> bool {
    let delta = std::array::from_fn(|axis| to[axis] - from[axis]);
    let min: [i32; 3] = std::array::from_fn(|axis| from[axis].min(to[axis]).floor() as i32);
    let max: [i32; 3] = std::array::from_fn(|axis| from[axis].max(to[axis]).floor() as i32);
    let below = i32::from(
        state
            .registry
            .max_collision_box_y
            .div_ceil(16)
            .saturating_sub(1),
    );
    for y in min[1].saturating_sub(below)..=max[1] {
        for z in min[2]..=max[2] {
            for x in min[0]..=max[0] {
                let Some(boxes) = state
                    .world
                    .block_at(x, y, z)
                    .and_then(|id| state.registry.collision_boxes.get(id as usize))
                else {
                    return false;
                };
                for collision_box in boxes {
                    let local = collision_box.as_blocks();
                    let offset = [f64::from(x), f64::from(y), f64::from(z)];
                    let lower = std::array::from_fn(|axis| offset[axis] + local[axis]);
                    let upper = std::array::from_fn(|axis| offset[axis] + local[axis + 3]);
                    if super::flight::segment_enters(from, delta, lower, upper).is_some() {
                        return false;
                    }
                }
            }
        }
    }
    true
}

fn stage_effect(
    state: &mut TickState,
    projectile: EntityId,
    target: EntityId,
    effect: ActiveEffect,
) {
    state
        .projectiles
        .after_hit
        .entry((projectile, target))
        .or_default()
        .push(effect);
}

/// Called only after direct damage admission. Hurting instant payloads also
/// enforce PvP here before returning magic pieces for armor/effect reduction,
/// preserving the strongest-hit window; healing remains independent of PvP.
pub(crate) fn accepted_hit(
    state: &mut TickState,
    target: EntityId,
    source: crate::intent::DamageSource,
    amount: &mut f32,
) -> Vec<crate::tick::DamagePiece> {
    let crate::intent::DamageSource::Projectile {
        projectile,
        owner,
        direct_type_id,
        ..
    } = source
    else {
        return Vec::new();
    };
    if !state.registry.advancements.definitions.is_empty()
        && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(direct_type_id)
            .is_some_and(|contract| contract.name == "minecraft:lightning_bolt")
        && let Some(record) = state
            .entities
            .slot_of(projectile)
            .and_then(|slot| state.entities.row(slot).projectile.as_deref())
        && record.kind == ProjectileKind::Lightning
        && let Some(actor) = owner_id(state, record).filter(|id| state.players.contains_key(id))
    {
        // Admission precedes health/death commit, so a lethal bolt still retains
        // the actual victim. The retained UUID, not a trident proposal or the
        // natural bolt's fallback damage owner, establishes player credit.
        let victim = crate::advancements::entity_snapshot(state, target);
        crate::advancements::trigger(
            state,
            actor,
            "minecraft:channeled_lightning",
            serde_json::json!({ "victims": [victim] }),
        );
    }
    if mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(direct_type_id)
        .is_some_and(|contract| contract.name == "minecraft:lightning_bolt")
        && crate::weather::lightning::strike_entity(state, target)
    {
        *amount = 0.0;
    }
    if let Some((position, owner)) = state
        .projectiles
        .channeling_hits
        .remove(&(projectile, target))
    {
        let lightning = ProjectileState::new(ProjectileKind::Lightning, owner, ItemStack::EMPTY);
        if let Some(id) = launch(state, lightning, position, [0.0; 3])
            && let Some(actor) = owner
                .and_then(|uuid| state.entities.slot_of_uuid(uuid))
                .map(|slot| state.entities.body(slot).id)
            && state.players.contains_key(&actor)
        {
            let lightning = crate::advancements::entity_snapshot(state, id);
            crate::advancements::trigger(
                state,
                actor,
                "minecraft:lightning_strike",
                serde_json::json!({ "lightning": lightning }),
            );
        }
    }
    let Some(effects) = state.projectiles.after_hit.remove(&(projectile, target)) else {
        return Vec::new();
    };
    let inverse = inverted(state, target);
    let mut damage = Vec::new();
    for effect in effects {
        if matches!(effect.id, EffectId::InstantHealth | EffectId::InstantDamage) {
            let hurting = (effect.id == EffectId::InstantDamage) != inverse;
            let amount = (if hurting { 6.0 } else { 4.0 })
                * 2.0_f32.powi(i32::from(effect.amplifier).min(30));
            if hurting {
                if !pvp_permitted(state, Some(owner), target) {
                    continue;
                }
                damage.push(crate::tick::DamagePiece {
                    amount,
                    kind: DamageKind::Magic,
                    source: None,
                    physical: None,
                    custom_armor: false,
                });
            } else {
                heal(state, target, amount);
            }
        } else {
            give_effect(state, target, effect);
        }
    }
    damage
}

/// The damage owner calls this only for an accepted lethal arrow hit, with the
/// victim snapshot captured before despawning. Hits alone are not kill evidence.
pub(crate) fn record_crossbow_kill(
    state: &mut TickState,
    projectile: EntityId,
    victim: serde_json::Value,
) {
    if state.registry.advancements.definitions.is_empty() {
        return;
    }
    let slot = state.entities.slot_of_any(projectile);
    let Some(record) = slot
        .and_then(|slot| state.entities.row(slot).projectile.as_deref())
        .or_else(|| state.projectiles.retired_crossbows.get(&projectile))
    else {
        return;
    };
    let Some(weapon) = record.fired_from_weapon.as_ref() else {
        return;
    };
    if !matches!(
        record.kind,
        ProjectileKind::Arrow | ProjectileKind::SpectralArrow
    ) {
        return;
    }
    let Some(actor) = owner_id(state, record).filter(|actor| state.players.contains_key(actor))
    else {
        return;
    };
    let weapon = crate::advancements::item_snapshot(state, weapon);
    let record = if let Some(slot) = slot {
        state
            .entities
            .row_mut(slot)
            .projectile
            .as_mut()
            .expect("projectile")
    } else {
        state
            .projectiles
            .retired_crossbows
            .get_mut(&projectile)
            .expect("retired crossbow")
    };
    record.crossbow_kills.push(victim);
    let victims = record.crossbow_kills.clone();
    let mut unique_entity_types = Some(0);
    for (index, victim) in victims.iter().enumerate() {
        let Some(kind) = victim
            .get("type")
            .and_then(serde_json::Value::as_str)
            .filter(|kind| !kind.is_empty())
        else {
            unique_entity_types = None;
            break;
        };
        if !victims[..index]
            .iter()
            .any(|old| old.get("type").and_then(serde_json::Value::as_str) == Some(kind))
        {
            *unique_entity_types.as_mut().expect("known entity types") += 1;
        }
    }
    let mut event = serde_json::json!({ "victims": victims, "fired_from_weapon": weapon });
    if let Some(count) = unique_entity_types {
        event["unique_entity_types"] = serde_json::json!(count);
    }
    if slot.is_some() {
        touch(state, projectile);
    }
    crate::advancements::trigger(state, actor, "minecraft:killed_by_arrow", event);
}

pub(super) fn level_event(state: &mut TickState, position: [f64; 3], event: i32, data: i32) {
    let recipients: Vec<_> = state
        .players
        .iter()
        .filter_map(|(id, player)| {
            let slot = state.entities.slot_of(*id)?;
            (distance_sq(state.entities.pos(slot), position) <= 4096.0).then_some(player.connection)
        })
        .collect();
    let position = BlockPos {
        x: position[0].floor() as i32,
        y: position[1].floor() as i32,
        z: position[2].floor() as i32,
    };
    for connection in recipients {
        state.push_player(
            connection,
            PlayerChange::ProjectileLevelEvent {
                position,
                event,
                data,
            },
        );
    }
}

fn firework_event(state: &mut TickState, id: EntityId, position: [f64; 3]) {
    let recipients: Vec<_> = state
        .players
        .iter()
        .filter_map(|(target, player)| {
            let slot = state.entities.slot_of(*target)?;
            (distance_sq(state.entities.pos(slot), position) <= 4096.0).then_some(player.connection)
        })
        .collect();
    for connection in recipients {
        state.push_player(
            connection,
            PlayerChange::ProjectileStatus {
                entity: id,
                event: 17,
            },
        );
    }
}

/// Populate only protocol layouts evidenced by OLD wire_entities.rs:418..497.
/// Arrow/trident metadata belongs to the flight owner.
pub(crate) fn initialize(state: &mut TickState, id: EntityId) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let Some(mut record) = state.entities.row_mut(slot).projectile.take() else {
        return;
    };
    if record.stack.is_empty() {
        let item = match record.kind {
            ProjectileKind::Potion => Some("minecraft:splash_potion"),
            ProjectileKind::LingeringPotion => Some("minecraft:lingering_potion"),
            ProjectileKind::Snowball => Some("minecraft:snowball"),
            ProjectileKind::Egg => Some("minecraft:egg"),
            ProjectileKind::EnderPearl => Some("minecraft:ender_pearl"),
            ProjectileKind::EyeOfEnder => Some("minecraft:ender_eye"),
            ProjectileKind::ExperienceBottle => Some("minecraft:experience_bottle"),
            ProjectileKind::Firework => Some("minecraft:firework_rocket"),
            _ => None,
        };
        if let Some(item_id) = item.and_then(|name| {
            state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(name).expect("built-in projectile item"))
        }) {
            record.stack = ItemStack::new(item_id, 1);
        }
    }
    if record.kind == ProjectileKind::AreaEffectCloud && matches!(record.data, ProjectileData::None)
    {
        record.data = ProjectileData::Cloud(CloudState::empty(state.tick));
    }
    if record.kind == ProjectileKind::Firework && matches!(record.data, ProjectileData::None) {
        let flight = record
            .stack
            .extended()
            .fireworks
            .as_ref()
            .map_or(0, |fireworks| fireworks.flight_duration.clamp(0, 255))
            as u32;
        let roll = random(id.0 as u64 ^ state.tick.saturating_sub(u64::from(record.age)));
        let lifetime = 10 * (1 + flight) + (roll % 6) as u32 + ((roll >> 16) % 7) as u32 + 1;
        record.data = ProjectileData::Firework(FireworkState { lifetime });
    }
    match record.kind {
        ProjectileKind::Snowball
        | ProjectileKind::Egg
        | ProjectileKind::EnderPearl
        // Local EyeOfEnder javap: Entity + first ITEM_STACK accessor (8).
        | ProjectileKind::EyeOfEnder
        | ProjectileKind::Potion
        | ProjectileKind::LingeringPotion
        | ProjectileKind::ExperienceBottle => {
            set_metadata(
                state,
                id,
                8,
                EntityDataValue::Item(Some(record.stack.clone())),
            );
        }
        ProjectileKind::Firework => {
            set_metadata(
                state,
                id,
                8,
                EntityDataValue::Item(Some(record.stack.clone())),
            );
            let attached = record
                .attached_to
                .and_then(|uuid| state.entities.slot_of_uuid(uuid))
                .map(|slot| state.entities.body(slot).id.0 as u32);
            set_metadata(state, id, 9, EntityDataValue::OptionalUnsignedInt(attached));
            set_metadata(
                state,
                id,
                10,
                EntityDataValue::Boolean(record.shot_at_angle),
            );
        }
        ProjectileKind::AreaEffectCloud => {
            if let ProjectileData::Cloud(cloud) = &record.data {
                cloud_metadata(state, id, cloud);
                // Canonical AreaEffectCloud default is EntityEffectParticle(-1).
                // OLD dragon clouds did not override it.
                set_metadata(
                    state,
                    id,
                    10,
                    EntityDataValue::EntityEffectParticle(
                        if cloud.dragon || record.stack.is_empty() {
                            -1
                        } else {
                            potion_color(&record.stack)
                        },
                    ),
                );
            }
        }
        _ => {}
    }
    state.entities.row_mut(slot).projectile = Some(record);
}

fn set_metadata(state: &mut TickState, id: EntityId, index: u8, value: EntityDataValue) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    if state
        .entities
        .row(slot)
        .appearance
        .extra
        .iter()
        .any(|(at, old)| *at == index && *old == value)
    {
        return;
    }
    let extra = &mut Arc::make_mut(&mut state.entities.row_mut(slot).appearance).extra;
    if let Some((_, old)) = extra.iter_mut().find(|(at, _)| *at == index) {
        *old = value;
    } else {
        extra.push((index, value));
    }
    touch(state, id);
}

/// Detached gliding rockets report a free attachment slot (vanilla index 9)
/// without touching the retained binding; flight decides detachment.
pub(super) fn clear_attachment(state: &mut TickState, id: EntityId) {
    set_metadata(state, id, 9, EntityDataValue::OptionalUnsignedInt(None));
}

fn cloud_metadata(state: &mut TickState, id: EntityId, cloud: &CloudState) {
    set_metadata(state, id, 8, EntityDataValue::Float(cloud.radius));
    set_metadata(state, id, 9, EntityDataValue::Boolean(cloud.waiting));
}

fn spawn_cloud(state: &mut TickState, record: &ProjectileState, position: [f64; 3], dragon: bool) {
    let mut cloud = ProjectileState::new(
        ProjectileKind::AreaEffectCloud,
        record.owner,
        record.stack.clone(),
    );
    cloud.data = ProjectileData::Cloud(if dragon {
        CloudState::dragon(state.tick)
    } else {
        CloudState::lingering(state.tick)
    });
    cloud.mob_damage = record.mob_damage;
    launch(state, cloud, position, [0.0; 3]);
}

fn pearl(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &ProjectileState,
    position: [f64; 3],
) {
    let Some(owner) = owner_id(state, record) else {
        return;
    };
    if state.entity_session_locked(owner) {
        return;
    }
    let Some(slot) = state.entities.slot_of(owner) else {
        return;
    };
    let Some(player) = state.players.get_mut(&owner) else {
        return;
    };
    if player.vitals.is_dead() {
        return;
    }
    player.fall_distance = 0.0;
    state.entities.body_mut(slot).burn_ticks = 0;
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).shared_flags &= !1;
    let connection = player.connection;
    let rotation = state.entities.rot(slot);
    crate::mounts::dismount(state, owner);
    state.entities.set_pos(slot, position);
    state.entities.set_ground(slot, false);
    state.correct_position(connection, position, rotation[0], rotation[1]);
    touch(state, owner);
    damage(
        state,
        intents,
        id,
        owner,
        5.0,
        DamageKind::Fall,
        None,
        [0.0; 3],
    );
}

fn hatch(state: &mut TickState, id: EntityId, position: [f64; 3], age: u32) {
    let Some(slot) = state.entities.slot_of(id) else {
        return;
    };
    let roll = random(
        state.entities.row(slot).uuid.as_u128() as u64 ^ state.tick.saturating_sub(u64::from(age)),
    );
    if !roll.is_multiple_of(8) {
        return;
    }
    let count = if random(roll).is_multiple_of(32) {
        4
    } else {
        1
    };
    let Some(type_id) = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:chicken").expect("chicken id"))
    else {
        return;
    };
    for _ in 0..count {
        let child = state.spawn_mob(type_id, position, 4.0);
        crate::entity_life::make_child(state, child);
        if let Some(slot) = state.entities.slot_of(child) {
            state.entities.row_mut(slot).breed_cooldown = state
                .registry
                .blueprint(type_id)
                .map_or(0, |blueprint| blueprint.params.breed_cooldown);
        }
        state.projectiles.spawned.push(child);
        touch(state, child);
    }
}

fn firework_damage(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &ProjectileState,
    position: [f64; 3],
) {
    firework_event(state, id, position);
    let count = record
        .stack
        .extended()
        .fireworks
        .as_ref()
        .map_or(0, |fireworks| fireworks.explosions.len());
    if count == 0 {
        return;
    }
    let base = 5.0 + 2.0 * count as f32;
    let owner = owner_id(state, record);
    for target in targets(state, position, 5.0) {
        if !pvp_permitted(state, owner, target) {
            continue;
        }
        let slot = state.entities.slot_of(target).expect("target");
        let at = state.entities.pos(slot);
        let middle = [at[0], at[1] + state.entities.body(slot).height * 0.5, at[2]];
        if !ray_clear(state, position, at) && !ray_clear(state, position, middle) {
            continue;
        }
        let amount = base
            * (1.0 - distance_sq(at, position).sqrt() / 5.0)
                .max(0.0)
                .sqrt() as f32;
        damage(
            state,
            intents,
            id,
            target,
            amount,
            DamageKind::Projectile,
            owner,
            [0.0; 3],
        );
    }
}

pub(crate) fn wind_burst(state: &mut TickState, position: [f64; 3], power: f32) {
    wind_burst_scaled(state, position, power, 1.0);
}

pub(crate) fn wind_burst_scaled(
    state: &mut TickState,
    position: [f64; 3],
    power: f32,
    multiplier: f64,
) {
    let radius = f64::from(power) * 2.0;
    let mut knockback = Vec::new();
    for target in targets(state, position, radius) {
        let slot = state.entities.slot_of(target).expect("target");
        let at = state.entities.pos(slot);
        let row = state.entities.body(slot);
        let min = [at[0] - row.half_width, at[1], at[2] - row.half_width];
        let max = [
            at[0] + row.half_width,
            at[1] + row.height,
            at[2] + row.half_width,
        ];
        let exposure = f64::from(crate::explosion::seen_percent(state, min, max, position));
        let distance = distance_sq(at, position).sqrt() / radius;
        let eye_height = if state.players.contains_key(&target) {
            1.62
        } else {
            row.height * 0.5
        };
        let delta = [
            at[0] - position[0],
            at[1] + eye_height - position[1],
            at[2] - position[2],
        ];
        let length = mth::length3(delta[0], delta[1], delta[2]);
        if length == 0.0 || distance > 1.0 || exposure == 0.0 {
            continue;
        }
        let impulse = delta.map(|value| value / length * (1.0 - distance) * exposure * multiplier);
        let velocity =
            std::array::from_fn(|axis| state.entities.vel(slot)[axis] + impulse[axis] * 20.0);
        state.entities.set_vel(slot, velocity);
        state.entities.set_ground(slot, false);
        if let Some(player) = state.players.get_mut(&target) {
            player.fall_distance = 0.0;
            player.current_impulse_impact_pos = Some(position);
            let connection = player.connection;
            state.push_player(
                connection,
                PlayerChange::Motion {
                    entity: target,
                    velocity,
                },
            );
            knockback.push((target, impulse));
        } else {
            let prev = state.entities.horizontal_impulse(slot);
            state.entities.set_horizontal_impulse(
                slot,
                [prev[0] + impulse[0] * 20.0, prev[1] + impulse[2] * 20.0],
            );
        }
        touch(state, target);
    }
    state.hostile.explosions.push(ExplosionEvent {
        center: position,
        radius: power,
        blocks: 0,
        knockback,
    });
}

/// Trident's caller has already established an entity hit and channeling level.
pub(crate) fn channeling(
    state: &mut TickState,
    intents: &[Intent],
    id: EntityId,
    record: &ProjectileState,
    position: [f64; 3],
) -> bool {
    if !record.stack.enchantments.iter().any(|enchantment| {
        enchantment.id.as_str() == "minecraft:channeling" && enchantment.level > 0
    }) {
        return false;
    }
    if !state.dimension.has_weather() || state.weather.0 != crate::commands::Weather::Thunder {
        return false;
    }
    let x = position[0].floor() as i32;
    let z = position[2].floor() as i32;
    if !(position[1].floor() as i32..state.dimension.geometry().max_y()).all(|y| {
        state.world.block_at(x, y, z).is_some_and(|block| {
            state.registry.materials.classify(block) == mc_physics::BlockMaterial::Air
        })
    }) {
        return false;
    }
    let Some(target) = intents.last().and_then(|intent| match intent.kind {
        IntentKind::DamageEntity { target, .. } if intent.actor == id => Some(target),
        _ => None,
    }) else {
        return false;
    };
    state
        .projectiles
        .channeling_hits
        .insert((id, target), (position, record.owner));
    true
}

/// Returns true when flight must remove the projectile after this impact.
pub(crate) fn impact(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &mut ProjectileState,
    hit: Option<EntityId>,
    position: [f64; 3],
) -> bool {
    let owner = owner_id(state, record);
    match record.kind {
        ProjectileKind::EnderPearl => pearl(state, intents, id, record, position),
        ProjectileKind::Egg => hatch(state, id, position, record.age),
        ProjectileKind::Snowball => {
            if let Some(target) = hit {
                let velocity = state
                    .entities
                    .slot_of(id)
                    .map_or([0.0; 3], |slot| state.entities.vel(slot).map(|v| v / 20.0));
                if entity_name(state, target) == Some("minecraft:blaze") {
                    damage(
                        state,
                        intents,
                        id,
                        target,
                        3.0,
                        DamageKind::Projectile,
                        owner,
                        velocity,
                    );
                } else if let Some(slot) = state.entities.slot_of(target) {
                    state.commit_hit_knockback(
                        slot,
                        crate::intent::DamageSource::Projectile {
                            owner: owner.unwrap_or(id),
                            projectile: id,
                            direct_type_id: 0,
                            velocity: [velocity[0], velocity[2]],
                            punch: 0,
                            flame: false,
                        },
                        crate::combat::HIT_KNOCKBACK,
                    );
                }
            }
        }
        ProjectileKind::Potion => {
            level_event(state, position, 2002, potion_color(&record.stack));
            for target in targets(state, position, 4.0) {
                let slot = state.entities.slot_of(target).expect("target");
                let scale = if hit == Some(target) {
                    1.0
                } else {
                    1.0 - distance_sq(position, state.entities.pos(slot)).sqrt() / 4.0
                };
                apply_potion(state, intents, id, record, target, scale, false);
            }
        }
        ProjectileKind::LingeringPotion => {
            level_event(state, position, 2002, potion_color(&record.stack));
            spawn_cloud(state, record, position, false);
        }
        ProjectileKind::DragonFireball => spawn_cloud(state, record, position, true),
        ProjectileKind::Firework => {
            if record.attached_to.is_some()
                && hit.is_some_and(|hit| Some(hit) == owner_id(state, record))
            {
                return false;
            }
            if hit.is_none()
                && record
                    .stack
                    .extended()
                    .fireworks
                    .as_ref()
                    .is_none_or(|fireworks| fireworks.explosions.is_empty())
            {
                return false;
            }
            firework_damage(state, intents, id, record, position);
        }
        ProjectileKind::ExperienceBottle => {
            let Some(slot) = state.entities.slot_of(id) else {
                return true;
            };
            let roll =
                random(state.entities.row(slot).uuid.as_u128() as u64 ^ u64::from(record.age));
            let amount = 3 + (roll % 5) as i32 + (random(roll) % 5) as i32;
            let mut spawned = Vec::new();
            crate::experience_orbs::award(state, position, amount, &mut spawned);
            state.projectiles.spawned.extend(spawned);
            level_event(state, position, 2002, -13_083_194);
        }
        ProjectileKind::SmallFireball
        | ProjectileKind::Fireball
        | ProjectileKind::WitherSkull
        | ProjectileKind::WindCharge
        | ProjectileKind::BreezeWindCharge
        | ProjectileKind::ShulkerBullet
        | ProjectileKind::LlamaSpit => {
            if let Some(target) = hit.filter(|target| {
                pvp_permitted(state, owner, *target)
                    && !(matches!(
                        record.kind,
                        ProjectileKind::WindCharge | ProjectileKind::BreezeWindCharge
                    ) && entity_name(state, *target) == Some("minecraft:breeze"))
            }) {
                let mut amount = match record.kind {
                    ProjectileKind::SmallFireball => 5.0,
                    ProjectileKind::Fireball => 6.0,
                    ProjectileKind::WitherSkull => 8.0,
                    ProjectileKind::ShulkerBullet => 4.0,
                    _ => 1.0,
                };
                if record.kind == ProjectileKind::SmallFireball {
                    let resistant = state.players.get(&target).map_or_else(
                        || {
                            state.entities.slot_of(target).is_some_and(|slot| {
                                state
                                    .entities
                                    .row(slot)
                                    .active_effects
                                    .has(EffectId::FireResistance)
                            })
                        },
                        |player| player.active_effects.has(EffectId::FireResistance),
                    );
                    if resistant {
                        amount = 0.0;
                    }
                }
                if record.mob_damage && state.players.contains_key(&target) {
                    amount = state.difficulty.scale_mob_damage(amount);
                }
                let velocity = state.entities.slot_of(id).map_or([0.0; 3], |slot| {
                    state.entities.vel(slot).map(|value| value / 20.0)
                });
                damage(
                    state,
                    intents,
                    id,
                    target,
                    amount,
                    if matches!(
                        record.kind,
                        ProjectileKind::SmallFireball | ProjectileKind::Fireball
                    ) {
                        DamageKind::Fireball
                    } else {
                        DamageKind::Projectile
                    },
                    owner,
                    velocity,
                );
                if record.kind == ProjectileKind::SmallFireball
                    && let Some(Intent {
                        kind:
                            IntentKind::DamageEntity {
                                source: Some(crate::intent::DamageSource::Projectile { flame, .. }),
                                ..
                            },
                        ..
                    }) = intents.last_mut()
                {
                    *flame = true;
                }
                if record.kind == ProjectileKind::ShulkerBullet {
                    stage_effect(
                        state,
                        id,
                        target,
                        ActiveEffect::new(EffectId::Levitation, 200, 0),
                    );
                }
            }
            if matches!(
                record.kind,
                ProjectileKind::Fireball | ProjectileKind::WitherSkull
            ) {
                let seq = state.next_seq(id);
                let mob_grief = record.mob_damage.then_some(match record.kind {
                    ProjectileKind::WitherSkull => crate::command_rules::MobGrief::Wither,
                    _ => crate::command_rules::MobGrief::Ghast,
                });
                intents.push(Intent {
                    actor: id,
                    seq,
                    kind: IntentKind::Explode {
                        center: position,
                        power: 1.0,
                        mob_grief,
                        kill_credit: owner.and_then(|owner| {
                            crate::statistics::living_type(state, owner)
                                .map(|type_id| (owner, type_id))
                        }),
                    },
                    footprint: Footprint::default(),
                });
            } else if matches!(
                record.kind,
                ProjectileKind::WindCharge | ProjectileKind::BreezeWindCharge
            ) {
                wind_burst(state, position, 3.0);
            }
        }
        ProjectileKind::AreaEffectCloud | ProjectileKind::Lightning => return false,
        _ => return false,
    }
    true
}

/// Returns true when this payload owns the tick (stationary cloud/lightning or
/// a rocket that detonated), and flight must not move it again.
pub(crate) fn step(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    record: &mut ProjectileState,
) -> bool {
    match record.kind {
        ProjectileKind::AreaEffectCloud => {
            let position = state
                .entities
                .slot_of(id)
                .map(|slot| state.entities.pos(slot));
            let Some(position) = position else {
                return true;
            };
            let ProjectileData::Cloud(mut cloud) =
                std::mem::replace(&mut record.data, ProjectileData::None)
            else {
                remove(state, id);
                return true;
            };
            if state.tick >= cloud.expires_tick
                || cloud.duration != u32::MAX
                    && record.age >= cloud.duration.saturating_add(cloud.wait_time)
            {
                remove(state, id);
                return true;
            }
            cloud.waiting = record.age < cloud.wait_time;
            if cloud.dragon {
                cloud.radius += cloud.radius_per_tick;
            }
            if state.tick >= cloud.next_tick && !cloud.waiting {
                if !cloud.dragon {
                    cloud.radius += cloud.radius_per_tick * 5.0;
                }
                cloud.affected_until.retain(|_, until| state.tick < *until);
                if cloud.radius < 0.5 {
                    remove(state, id);
                    return true;
                }
                let owner = owner_id(state, record);
                for target in cloud_targets(state, position, &cloud) {
                    let slot = state.entities.slot_of(target).expect("target");
                    let at = state.entities.pos(slot);
                    let uuid = state.entities.row(slot).uuid;
                    let overlaps = if cloud.dragon {
                        at[1] <= position[1] + 0.5
                            && at[1] + state.entities.body(slot).height >= position[1]
                    } else {
                        (at[1] - position[1]).abs() <= 0.5
                    };
                    if !overlaps
                        || cloud.affected_until.contains_key(&uuid)
                        || !pvp_permitted(state, owner, target)
                        || cloud.dragon && owner == Some(target)
                    {
                        continue;
                    }
                    if cloud.dragon {
                        damage(
                            state,
                            intents,
                            id,
                            target,
                            6.0,
                            DamageKind::Magic,
                            owner,
                            [0.0; 3],
                        );
                    } else {
                        apply_potion(state, intents, id, record, target, 1.0, true);
                    }
                    cloud.affected_until.insert(
                        uuid,
                        state
                            .tick
                            .saturating_add(u64::from(cloud.reapplication_delay)),
                    );
                    cloud.radius += cloud.radius_on_use;
                    if cloud.radius < 0.5 {
                        break;
                    }
                }
                cloud.next_tick = state.tick.saturating_add(5);
            }
            if cloud.radius < 0.5 {
                remove(state, id);
                return true;
            }
            cloud_metadata(state, id, &cloud);
            record.data = ProjectileData::Cloud(cloud);
            touch(state, id);
            true
        }
        ProjectileKind::Firework => {
            let lifetime = match &record.data {
                ProjectileData::Firework(firework) => firework.lifetime,
                _ => return false,
            };
            if record.age < lifetime {
                return false;
            }
            if let Some(slot) = state.entities.slot_of(id) {
                firework_damage(state, intents, id, record, state.entities.pos(slot));
            }
            remove(state, id);
            true
        }
        ProjectileKind::Lightning => {
            if record.age <= 1
                && let Some(slot) = state.entities.slot_of(id)
            {
                let position = state.entities.pos(slot);
                crate::weather::lightning::strike_blocks(state, position);
                let owner = owner_id(state, record);
                for target in lightning_targets(state, position) {
                    if pvp_permitted(state, owner, target) {
                        let amount = if entity_name(state, target) == Some("minecraft:turtle") {
                            f32::MAX
                        } else {
                            5.0
                        };
                        damage(
                            state,
                            intents,
                            id,
                            target,
                            amount,
                            DamageKind::Lightning,
                            owner,
                            [0.0; 3],
                        );
                    }
                }
            }
            if record.age >= 20 {
                remove(state, id);
            }
            true
        }
        _ => false,
    }
}

#[cfg(test)]
#[path = "payloads_tests.rs"]
mod tests;
