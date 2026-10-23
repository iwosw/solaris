//! Retained native projectiles: one flight authority for mobs, items and dispensers.

pub(crate) mod embedded_projectiles;
pub(crate) mod eye;

pub mod fishing;
pub(crate) mod flight;
#[cfg(test)]
mod flight_tests;
#[cfg(test)]
mod eye_tests;
pub mod payloads;
pub mod persistence;
pub mod weapons;

use std::collections::HashMap;

use mc_data::ItemStack;
use uuid::Uuid;

use crate::blueprint::{
    Goal, GoalKind, MobBlueprint, MobCategory, MobParams, MovementClass, Sensors,
};
use crate::boundary::EntityId;
use crate::entities::EntitySlot;
use crate::intent::{DamageSource, Footprint, Intent, IntentKind};
use crate::survival::DamageKind;
use crate::tick::TickState;

pub const ARROW_IN_GROUND_INDEX: u8 = 10;
pub const ARROW_STUCK_TICKS: u32 = 1200;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ProjectileKind {
    Arrow,
    SpectralArrow,
    Trident,
    Snowball,
    Egg,
    EnderPearl,
    Potion,
    LingeringPotion,
    ExperienceBottle,
    Firework,
    FishingHook,
    AreaEffectCloud,
    SmallFireball,
    Fireball,
    DragonFireball,
    WitherSkull,
    WindCharge,
    BreezeWindCharge,
    ShulkerBullet,
    LlamaSpit,
    Lightning,
    EyeOfEnder,
}

impl ProjectileKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Arrow => "minecraft:arrow",
            Self::SpectralArrow => "minecraft:spectral_arrow",
            Self::Trident => "minecraft:trident",
            Self::Snowball => "minecraft:snowball",
            Self::Egg => "minecraft:egg",
            Self::EnderPearl => "minecraft:ender_pearl",
            Self::Potion => "minecraft:splash_potion",
            Self::LingeringPotion => "minecraft:lingering_potion",
            Self::ExperienceBottle => "minecraft:experience_bottle",
            Self::Firework => "minecraft:firework_rocket",
            Self::FishingHook => "minecraft:fishing_bobber",
            Self::AreaEffectCloud => "minecraft:area_effect_cloud",
            Self::SmallFireball => "minecraft:small_fireball",
            Self::Fireball => "minecraft:fireball",
            Self::DragonFireball => "minecraft:dragon_fireball",
            Self::WitherSkull => "minecraft:wither_skull",
            Self::WindCharge => "minecraft:wind_charge",
            Self::BreezeWindCharge => "minecraft:breeze_wind_charge",
            Self::ShulkerBullet => "minecraft:shulker_bullet",
            Self::LlamaSpit => "minecraft:llama_spit",
            Self::Lightning => "minecraft:lightning_bolt",
            Self::EyeOfEnder => "minecraft:eye_of_ender",
        }
    }
    pub fn from_name(name: &str) -> Option<Self> {
        ALL_KINDS.iter().copied().find(|kind| kind.name() == name)
    }
    pub(crate) fn arrow(self) -> bool {
        matches!(self, Self::Arrow | Self::SpectralArrow | Self::Trident)
    }
}

const ALL_KINDS: &[ProjectileKind] = &[
    ProjectileKind::Arrow,
    ProjectileKind::SpectralArrow,
    ProjectileKind::Trident,
    ProjectileKind::Snowball,
    ProjectileKind::Egg,
    ProjectileKind::EnderPearl,
    ProjectileKind::Potion,
    ProjectileKind::LingeringPotion,
    ProjectileKind::ExperienceBottle,
    ProjectileKind::Firework,
    ProjectileKind::FishingHook,
    ProjectileKind::AreaEffectCloud,
    ProjectileKind::SmallFireball,
    ProjectileKind::Fireball,
    ProjectileKind::DragonFireball,
    ProjectileKind::WitherSkull,
    ProjectileKind::WindCharge,
    ProjectileKind::BreezeWindCharge,
    ProjectileKind::ShulkerBullet,
    ProjectileKind::LlamaSpit,
    ProjectileKind::Lightning,
    ProjectileKind::EyeOfEnder,
];

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ProjectileData {
    None,
    Fishing(fishing::FishingState),
    Cloud(payloads::CloudState),
    Firework(payloads::FireworkState),
    /// Native authored projectile identity; requires the matching physical WAL image.
    Physical {
        definition: mc_data::Identifier,
    },
    /// UUID survives allocator remapping and offline/rejoined targets.
    Homing {
        target: Uuid,
    },
    Eye {
        origin: [f64; 3],
        target: [f64; 3],
        drop_item: bool,
    },
}

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ProjectileState {
    pub owner: Option<Uuid>,
    pub kind: ProjectileKind,
    pub age: u32,
    pub stuck: bool,
    pub pickup: bool,
    pub stack: ItemStack,
    pub damage: f32,
    pub punch: u32,
    pub flame: bool,
    pub piercing: u32,
    pub hit: Vec<Uuid>,
    pub data: ProjectileData,
    #[serde(default)]
    pub critical: bool,
    #[serde(default)]
    pub mob_damage: bool,
    #[serde(default)]
    pub returning: bool,
    #[serde(default)]
    pub shot_at_angle: bool,
    /// Actual weapon custody at launch, never inferred from the owner's held item.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fired_from_weapon: Option<std::sync::Arc<ItemStack>>,
    /// Confirmed deaths from this projectile, retained for piercing criteria.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crossbow_kills: Vec<serde_json::Value>,
    /// Runtime-only gliding attachment, like vanilla FireworkRocketEntity.
    /// UUID avoids stale wire ids when a live player connection is replaced.
    #[serde(skip)]
    pub attached_to: Option<Uuid>,
    #[serde(default)]
    pub acceleration_power: f64,
}

impl ProjectileState {
    pub fn new(kind: ProjectileKind, owner: Option<Uuid>, mut stack: ItemStack) -> Self {
        if !stack.is_empty() {
            stack.count = 1;
        }
        Self {
            owner,
            kind,
            age: 0,
            stuck: false,
            pickup: false,
            stack,
            damage: if kind == ProjectileKind::Trident {
                8.0
            } else {
                2.0
            },
            punch: 0,
            flame: false,
            piercing: 0,
            hit: Vec::new(),
            data: ProjectileData::None,
            critical: false,
            mob_damage: false,
            returning: false,
            shot_at_angle: false,
            fired_from_weapon: None,
            crossbow_kills: Vec::new(),
            attached_to: None,
            acceleration_power: if matches!(
                kind,
                ProjectileKind::SmallFireball
                    | ProjectileKind::Fireball
                    | ProjectileKind::DragonFireball
                    | ProjectileKind::WitherSkull
            ) {
                0.1
            } else {
                0.0
            },
        }
    }
}

#[derive(Debug, Default)]
pub struct ProjectileRuntime {
    pub(crate) using: HashMap<EntityId, weapons::WeaponUse>,
    pub(crate) after_hit: HashMap<(EntityId, EntityId), Vec<crate::effects::ActiveEffect>>,
    pub(crate) channeling_hits: HashMap<(EntityId, EntityId), ([f64; 3], Option<Uuid>)>,
    pub(crate) embedded_hits: rustc_hash::FxHashSet<(EntityId, EntityId)>,
    pub(crate) spawned: Vec<EntityId>,
    pub(crate) removed: Vec<EntityId>,
    /// Entity-hit arrows stay addressable until shield admission can deflect them.
    pub(crate) pending_arrow_removals: rustc_hash::FxHashSet<EntityId>,
    pub(crate) riptides: HashMap<EntityId, weapons::Riptide>,
    /// Removed projectiles retain launch evidence until same-tick damage admission.
    pub(crate) retired_crossbows: HashMap<EntityId, ProjectileState>,
    bodies: Vec<flight::Hittable>,
}

pub(crate) fn item_name<'a>(state: &'a TickState, stack: &ItemStack) -> Option<&'a str> {
    state
        .registry
        .items
        .name_of(stack.item_id)
        .map(mc_data::Identifier::as_str)
}

pub(crate) fn owner_id(state: &TickState, projectile: &ProjectileState) -> Option<EntityId> {
    state
        .entities
        .slot_of_uuid(projectile.owner?)
        .map(|slot| state.entities.body(slot).id)
}

pub(crate) fn launch(
    state: &mut TickState,
    record: ProjectileState,
    origin: [f64; 3],
    velocity: [f64; 3],
) -> Option<EntityId> {
    if origin
        .iter()
        .chain(velocity.iter())
        .any(|value| !value.is_finite())
    {
        return None;
    }
    let type_id = state
        .registry
        .blueprints
        .iter()
        .find(|(_, blueprint)| blueprint.name == record.kind.name())
        .map(|(id, _)| *id)?;
    let id = state.spawn_mob(type_id, origin, 1.0);
    let slot = state.entities.slot_of(id)?;
    initialize_appearance(state, slot, &record);
    state.entities.row_mut(slot).projectile = Some(Box::new(record));
    set_flight(state, slot, velocity);
    payloads::initialize(state, id);
    state.projectiles.spawned.push(id);
    touch(state, id);
    Some(id)
}

pub(crate) fn initialize_appearance(
    state: &mut TickState,
    slot: EntitySlot,
    record: &ProjectileState,
) {
    let id = state.entities.body(slot).id;
    let owner = owner_id(state, record).map_or(0, |owner| owner.0);
    let data = if record.kind == ProjectileKind::FishingHook && owner == 0 {
        id.0
    } else if record.kind == ProjectileKind::EyeOfEnder {
        0
    } else {
        owner
    };
    if state.entities.row(slot).appearance.spawn_data != data {
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).spawn_data = data;
    }
    if record.kind.arrow() {
        // AbstractArrow javap: BYTE flags/PIERCE_LEVEL followed by BOOLEAN
        // IN_GROUND; critical mask=1, no-physics mask=2 (loyalty return).
        let flags = i8::from(record.critical) | (i8::from(record.returning) << 1);
        set_extra(
            state,
            slot,
            8,
            crate::boundary::EntityDataValue::Byte(flags),
        );
        set_extra(
            state,
            slot,
            9,
            crate::boundary::EntityDataValue::Byte(record.piercing.min(127) as i8),
        );
        set_extra(
            state,
            slot,
            10,
            crate::boundary::EntityDataValue::Boolean(record.stuck),
        );
    }
    if record.kind == ProjectileKind::Trident {
        let loyalty = crate::enchantments::level(&record.stack, "loyalty").min(127) as i8;
        set_extra(
            state,
            slot,
            11,
            crate::boundary::EntityDataValue::Byte(loyalty),
        );
        set_extra(
            state,
            slot,
            12,
            crate::boundary::EntityDataValue::Boolean(!record.stack.enchantments.is_empty()),
        );
    }
    if record.kind == ProjectileKind::Firework {
        let attached = record
            .attached_to
            .and_then(|uuid| state.entities.slot_of_uuid(uuid))
            .filter(|slot| state.entities.body(*slot).alive)
            .map(|slot| state.entities.body(slot).id.0 as u32);
        set_extra(
            state,
            slot,
            9,
            crate::boundary::EntityDataValue::OptionalUnsignedInt(attached),
        );
    }
}

fn set_extra(
    state: &mut TickState,
    slot: EntitySlot,
    index: u8,
    value: crate::boundary::EntityDataValue,
) {
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
    let extra = &mut std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).extra;
    if let Some((_, old)) = extra.iter_mut().find(|(at, _)| *at == index) {
        *old = value;
    } else {
        extra.push((index, value));
    }
}

pub(crate) fn touch(state: &mut TickState, id: EntityId) {
    state.appearance_changed.push(id);
}

pub(crate) fn remove(state: &mut TickState, id: EntityId) {
    if let Some(slot) = state.entities.slot_of_any(id) {
        state.remove_entity(slot);
        state.projectiles.removed.push(id);
    }
}

/// Queue owned pearls with the death commit's removals before it drops the
/// owner's UUID. Do not swap-remove here: damage and pickup transactions still
/// use dense slots, and the commit removes the combined list in descending order.
pub(crate) fn queue_owned_pearls_on_deaths(
    state: &TickState,
    deaths: &[EntityId],
    removals: &mut Vec<EntitySlot>,
) {
    if deaths.is_empty() || !crate::command_rules::boolean(state, "ender_pearls_vanish_on_death") {
        return;
    }
    let owners: rustc_hash::FxHashSet<Uuid> = deaths
        .iter()
        .filter_map(|id| state.entities.slot_of_any(*id))
        .map(|slot| state.entities.row(slot).uuid)
        .collect();
    if owners.is_empty() {
        return;
    }
    // One population pass for the entire death batch, not one scan per owner.
    removals.extend(state.entities.slots().filter(|&slot| {
        state
            .entities
            .row(slot)
            .projectile
            .as_ref()
            .is_some_and(|record| {
                record.kind == ProjectileKind::EnderPearl
                    && record.owner.is_some_and(|uuid| owners.contains(&uuid))
            })
    }));
}

pub(crate) fn set_flight(state: &mut TickState, slot: EntitySlot, velocity: [f64; 3]) {
    state.entities.set_vel(slot, velocity.map(|v| v * 20.0));
    state.entities.set_rot(
        slot,
        mc_physics::mth::atan2(velocity[0], velocity[2]) as f32 * mc_physics::mth::RAD_TO_DEG,
        mc_physics::mth::atan2(
            velocity[1],
            mc_physics::mth::length2(velocity[0], velocity[2]),
        ) as f32
            * mc_physics::mth::RAD_TO_DEG,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn damage(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    id: EntityId,
    target: EntityId,
    amount: f32,
    kind: DamageKind,
    owner: Option<EntityId>,
    velocity: [f64; 3],
) {
    let seq = state.next_seq(id);
    let direct_type_id = state
        .entities
        .slot_of(id)
        .and_then(|slot| match state.entities.body(slot).kind {
            crate::entities::EntityKind::Mob { type_id } => Some(type_id),
            _ => None,
        })
        .unwrap_or(0);
    intents.push(Intent {
        actor: id,
        seq,
        kind: IntentKind::DamageEntity {
            target,
            amount,
            kind,
            source: (kind != DamageKind::Fall).then_some(DamageSource::Projectile {
                owner: owner.unwrap_or(id),
                projectile: id,
                direct_type_id,
                velocity: [velocity[0], velocity[2]],
                punch: 0,
                flame: false,
            }),
        },
        footprint: Footprint {
            entity_writes: vec![target],
            ..Default::default()
        },
    });
}

pub(crate) fn run(
    state: &mut TickState,
    intents: &mut Vec<Intent>,
    specials: &[(EntityId, crate::hostile::Special)],
) {
    state.projectiles.after_hit.clear();
    state.projectiles.channeling_hits.clear();
    state.projectiles.embedded_hits.clear();
    weapons::tick(state, intents);
    flight::run(
        state,
        intents,
        specials.iter().filter_map(|(id, special)| {
            (*special == crate::hostile::Special::Projectile).then_some(*id)
        }),
    );
}

pub(crate) fn dispense(
    state: &mut TickState,
    item: &ItemStack,
    origin: [f64; 3],
    direction: [f64; 3],
) -> bool {
    let Some(kind) = item_name(state, item).and_then(|name| match name {
        "minecraft:arrow" | "minecraft:tipped_arrow" => Some(ProjectileKind::Arrow),
        "minecraft:spectral_arrow" => Some(ProjectileKind::SpectralArrow),
        "minecraft:snowball" => Some(ProjectileKind::Snowball),
        "minecraft:egg" => Some(ProjectileKind::Egg),
        "minecraft:ender_pearl" => Some(ProjectileKind::EnderPearl),
        "minecraft:experience_bottle" => Some(ProjectileKind::ExperienceBottle),
        "minecraft:splash_potion" => Some(ProjectileKind::Potion),
        "minecraft:lingering_potion" => Some(ProjectileKind::LingeringPotion),
        "minecraft:fire_charge" => Some(ProjectileKind::SmallFireball),
        "minecraft:firework_rocket" => Some(ProjectileKind::Firework),
        _ => None,
    }) else {
        return false;
    };
    let mut record = ProjectileState::new(kind, None, item.clone());
    record.pickup = kind.arrow();
    let speed = if matches!(
        kind,
        ProjectileKind::Potion | ProjectileKind::LingeringPotion
    ) {
        0.55
    } else {
        1.1
    };
    launch(state, record, origin, direction.map(|value| value * speed)).is_some()
}

pub(crate) fn extend_blueprints(
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    out: &mut Vec<(u32, MobBlueprint)>,
) {
    const GOALS: &[Goal] = &[Goal {
        priority: 0,
        kind: GoalKind::Projectile,
    }];
    for &kind in ALL_KINDS {
        if out
            .iter()
            .any(|(_, blueprint)| blueprint.name == kind.name())
        {
            continue;
        }
        let id = mc_data::Identifier::parse(kind.name()).expect("static entity identifier");
        let Some(type_id) = entity_types.id_of(&id) else {
            continue;
        };
        let Some(facts) = entity_types.facts_of(&id) else {
            continue;
        };
        let half_width = facts.dimensions.width / 2.0;
        let height = facts.dimensions.height;
        out.push((
            type_id,
            MobBlueprint {
                name: kind.name().to_owned(),
                type_id,
                movement: MovementClass::Projectile,
                goals: GOALS.to_vec(),
                sensors: Sensors {
                    sight_range: 0.0,
                    attack_reach: 0.0,
                },
                params: MobParams {
                    max_health: 1.0,
                    speed: 0.0,
                    attack_damage: 0.0,
                    pickup_delay: 0,
                    breed_cooldown: 0,
                    armor: 0.0,
                    knockback_resistance: 0.0,
                },
                half_width,
                height,
                offspring: None,
                breed_food: Vec::new(),
                loot_table: None,
                category: MobCategory::Misc,
                wool: false,
                spawn_weight: 0,
                capabilities: crate::blueprint::Capabilities::default(),
            },
        ));
    }
}
