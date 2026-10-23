//! Mob blueprints as data (ADR 0013 "Mob blueprints", ADR 0016 capabilities).
//!
//! A mob type, vanilla or data-defined, is an owned blueprint: movement class,
//! prioritized goals, sensors, parameters and [`Capabilities`]. The entity
//! phase executes blueprints natively; a behaviour several types share is a
//! capability read from the blueprint, never a type-name branch. Built-in
//! vanilla blueprints stay Rust data ([`vanilla_blueprints`]); data-defined
//! blueprints enter through [`MobBlueprintDef::admit`] and fail closed on any
//! field outside its [`limits`].

use std::ops::RangeInclusive;

use crate::species::AquaticControl;

/// How the entity phase moves this mob. Selects a [`mc_physics`]
/// [`PhysicsConfig`](mc_physics::PhysicsConfig), never a type-name branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MovementClass {
    /// Terrestrial walker with gravity, friction, step height.
    Walker,
    /// Aquatic swimmer.
    Swimmer,
    /// Free-flight steering with collision and no terrestrial gravity.
    Flyer,
    /// Ground locomotion on land and swimming when immersed.
    Amphibious,
    /// Living stationary baseline; no autonomous locomotion.
    Stationary,
    /// Item entity: gravity + drag, no AI.
    Item,
    /// Player-controlled: validated client movement, no AI.
    Player,
    /// Ballistic projectile (arrow): flies, sticks, no steering.
    Projectile,
}

/// One steering goal with a priority. Lower number runs first; the first
/// goal that produces an intent wins for this tick.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Goal {
    /// Priority; evaluated in ascending order.
    pub priority: u8,
    /// What the goal does.
    pub kind: GoalKind,
}

/// Goal behaviours the entity phase executes natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GoalKind {
    /// Flee when burning (all living mobs).
    FleeFire,
    /// Melee-chase the nearest visible player within follow range.
    MeleeChase,
    /// Melee attack when in reach.
    MeleeAttack,
    /// Wander to a random nearby point.
    Wander,
    /// Drift in water.
    Swim,
    /// Seek a breeding partner when in love; produce a child.
    Breed,
    /// Do nothing (item entities, players).
    Idle,
    /// Keep a bow's distance from the nearest visible player and shoot
    /// arrows at it (vanilla `RangedBowAttackGoal`).
    RangedAttack,
    /// Start a fuse next to a visible player and explode at the end
    /// (vanilla creeper `SwellGoal`); the blast is [`Capabilities::explodes`].
    Swell,
    /// Fly the ballistic arrow path.
    Projectile,
    /// Timed native ranged and auxiliary lifecycle goals.
    BlazeBurst,
    SiegeShot,
    Crossbow,
    Alchemist,
    WindCharge,
    Beam,
    ElderBeam,
    SonicBoom,
    Fangs,
    HomingShot,
    WitherBarrage,
    FangLife,
    VexLife,
}

/// What the mob can perceive this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sensors {
    /// Sight radius in blocks.
    pub sight_range: f32,
    /// Attack reach in blocks.
    pub attack_reach: f32,
}

/// Tunable parameters; every magic number for this mob lives here.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MobParams {
    /// Max health.
    pub max_health: f32,
    /// Walk speed in blocks/second.
    pub speed: f32,
    /// Melee damage per hit.
    pub attack_damage: f32,
    /// Ticks of pickup/merge immunity for fresh item entities.
    pub pickup_delay: u32,
    /// Breeding cooldown in ticks after giving birth.
    pub breed_cooldown: u32,
    /// Natural armor points before equipment (vanilla `armor` attribute).
    pub armor: f64,
    /// Knockback resistance before equipment, `0..=1`.
    pub knockback_resistance: f64,
}

/// Native behaviours a blueprint opts into. Each one is executed by exactly
/// one generic system for every blueprint that sets it (ADR 0016).
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Capabilities {
    /// Ignites under open sky by day (vanilla `isSunBurnTick` mobs).
    pub burns_in_daylight: bool,
    /// Hostile only at night: by day it ignores players (vanilla spiders).
    pub night_aggro_only: bool,
    /// One drowning damage per tick in water or rain (vanilla
    /// `isSensitiveToWater`).
    pub hurt_by_water: bool,
    /// Never owes fall damage (`#minecraft:fall_damage_immune`).
    pub fall_damage_immune: bool,
    /// Climbs a horizontal obstacle at 0.2 blocks per tick (vanilla spiders).
    pub climbs_walls: bool,
    /// Instant health harms and instant damage heals (vanilla undead).
    pub inverted_healing: bool,
    /// Swimmer steering family.
    pub aquatic: AquaticControl,
    /// Fuse length and blast of the [`GoalKind::Swell`] goal.
    pub explodes: Option<Explosion>,
    /// Follow players holding a breeding-food stack, without consuming it.
    pub temptable: bool,
    /// Juveniles follow nearby adults of their blueprint.
    pub follows_adults: bool,
    /// Reciprocal partner/offspring rule for a different blueprint.
    pub crossbreed: Option<Crossbreed>,
}

/// Both parents must declare the same offspring and each other's key.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Crossbreed {
    pub partner: u32,
    pub offspring: u32,
}

/// The blast a [`GoalKind::Swell`] fuse ends in.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Explosion {
    /// Blast power (vanilla creeper `explosionRadius` 3).
    pub power: f32,
    /// Fuse length in ticks (vanilla `maxSwell` 30).
    pub fuse_ticks: u32,
    /// Whether the blast may break blocks. When true the `mob_griefing`
    /// gamerule and the creeper block-damage switch still apply.
    pub griefing: bool,
}

/// Vanilla categories, caps and despawn distances share the data contract.
pub use mc_data::entity_types::MobCategory;

/// One mob type as data.
#[derive(Debug, Clone, PartialEq)]
pub struct MobBlueprint {
    /// Namespaced id, e.g. `minecraft:zombie` or `example:sky_creeper`.
    pub name: String,
    /// Blueprint key: the protocol entity type id of a vanilla blueprint, a
    /// [`CUSTOM_TYPE_FLAG`] key of a data-defined one (see [`wire_type_id`]).
    pub type_id: u32,
    /// Movement/physics class.
    pub movement: MovementClass,
    /// Goals sorted by priority ascending.
    pub goals: Vec<Goal>,
    /// Sensors.
    pub sensors: Sensors,
    /// Parameters.
    pub params: MobParams,
    /// Body dimensions for physics.
    pub half_width: f64,
    /// Body height for physics.
    pub height: f64,
    /// Offspring protocol type id for the Breed goal (`None` = no children).
    pub offspring: Option<u32>,
    /// Items that put this mob in love (Breed goal), resolved item ids.
    pub breed_food: Vec<u32>,
    /// Vanilla entity loot table rolled on death.
    pub loot_table: Option<mc_data::Identifier>,
    /// Natural spawn and despawn category.
    pub category: MobCategory,
    /// Grows wool (vanilla sheep): a color rolled at spawn rides the
    /// [`SHEEP_WOOL_DATA_INDEX`] entity data and picks the wool loot.
    pub wool: bool,
    /// Baseline relative spawn weight (`SpawnerData`); used for monsters.
    /// Creature weights and herd counts come from the resident biome table.
    pub spawn_weight: u32,
    /// Shared native behaviours.
    pub capabilities: Capabilities,
}

/// Keys of data-defined blueprints carry this bit. Bits 16..31 hold the
/// registration ordinal, bits 0..16 the vanilla protocol type the client is
/// shown: the protocol cannot add entity types.
pub const CUSTOM_TYPE_FLAG: u32 = 1 << 31;

/// Protocol entity type id clients see for a blueprint key.
#[must_use]
pub const fn wire_type_id(type_id: u32) -> u32 {
    if type_id & CUSTOM_TYPE_FLAG == 0 {
        type_id
    } else {
        type_id & 0xFFFF
    }
}

/// Bounds of every data-defined blueprint parameter (ADR 0016). Native
/// parameters spend core tick CPU that no plugin fuel pays for, so each one
/// is bounded at admission: search radius (target-grid cells per query),
/// speed (swept-collision length per tick), blast power (ray length and the
/// entity query radius) and fuse (blast cadence per exploder).
pub mod limits {
    use std::ops::RangeInclusive;

    /// Data-defined blueprints one registry admits.
    pub const MAX_CUSTOM_BLUEPRINTS: usize = 64;
    /// Goals one data-defined blueprint schedules.
    pub const MAX_GOALS: usize = 8;
    /// Highest goal priority.
    pub const MAX_GOAL_PRIORITY: u8 = 15;
    pub const MAX_HEALTH: RangeInclusive<f32> = 1.0..=1024.0;
    /// Blocks per second (0.8 blocks per tick).
    pub const SPEED: RangeInclusive<f32> = 0.0..=16.0;
    pub const ATTACK_DAMAGE: RangeInclusive<f32> = 0.0..=100.0;
    /// Blocks; four chunks, at most 9x9 target-grid cells per search.
    pub const SIGHT_RANGE: RangeInclusive<f32> = 0.0..=64.0;
    /// Blocks; also at most the sight range.
    pub const ATTACK_REACH: RangeInclusive<f32> = 0.0..=24.0;
    pub const ARMOR: RangeInclusive<f64> = 0.0..=30.0;
    pub const KNOCKBACK_RESISTANCE: RangeInclusive<f64> = 0.0..=1.0;
    /// Charged-creeper power at most.
    pub const EXPLOSION_POWER: RangeInclusive<f32> = 0.5..=6.0;
    pub const FUSE_TICKS: RangeInclusive<u32> = 10..=600;
    /// Presentation body half width in blocks: the contact-hazard scan
    /// envelope (`contact_hazards::contact_cells` clamps at 2.0).
    pub const BODY_HALF_WIDTH: RangeInclusive<f64> = 0.0..=2.0;
    /// Presentation body height in blocks: the same envelope (clamped at 6.0).
    pub const BODY_HEIGHT: RangeInclusive<f64> = 0.0..=6.0;
}

/// Presentation types whose client entity needs server-side state a
/// data-defined mob never gets: the client `EnderDragon` creates eight part
/// entities at wire ids parent+1..=8, which only `end_fight` reserves.
const REFUSED_PRESENTATIONS: [&str; 1] = ["minecraft:ender_dragon"];

/// Goals a data-defined blueprint may schedule: the generic executors that
/// need no per-type state.
const DATA_GOALS: [GoalKind; 8] = [
    GoalKind::FleeFire,
    GoalKind::MeleeChase,
    GoalKind::MeleeAttack,
    GoalKind::Wander,
    GoalKind::Swim,
    GoalKind::Idle,
    GoalKind::RangedAttack,
    GoalKind::Swell,
];

/// One data-defined mob type, read from JSON. Unknown fields are rejected;
/// [`MobBlueprintDef::admit`] enforces the [`limits`].
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobBlueprintDef {
    /// Namespaced id outside the `minecraft` namespace.
    pub id: mc_data::Identifier,
    /// Vanilla living type clients render; its hitbox and entity-data layout
    /// are the blueprint's body dimensions and wire metadata.
    pub presents_as: mc_data::Identifier,
    /// Vanilla `MobCategory` name (`monster`, `creature`, ...).
    pub category: String,
    pub movement: MovementClass,
    pub goals: Vec<Goal>,
    pub sight_range: f32,
    pub attack_reach: f32,
    pub max_health: f32,
    pub speed: f32,
    pub attack_damage: f32,
    #[serde(default)]
    pub armor: f64,
    #[serde(default)]
    pub knockback_resistance: f64,
    /// Vanilla entity loot table rolled on death.
    #[serde(default)]
    pub loot_table: Option<mc_data::Identifier>,
    #[serde(default)]
    pub capabilities: Capabilities,
}

fn bounded<T: PartialOrd + std::fmt::Display>(
    field: &str,
    value: T,
    range: &RangeInclusive<T>,
) -> Result<(), String> {
    if range.contains(&value) {
        Ok(())
    } else {
        Err(format!(
            "{field} {value} outside {}..={}",
            range.start(),
            range.end()
        ))
    }
}

impl MobBlueprintDef {
    /// Parse one definition; unknown fields and wrong types are errors.
    pub fn from_json(text: &str) -> Result<Self, String> {
        serde_json::from_str(text).map_err(|error| format!("mob blueprint: {error}"))
    }

    /// Validate against the [`limits`] and build the blueprint with key
    /// `ordinal`. Any violation is a load error naming the field.
    pub(crate) fn admit(
        mut self,
        ordinal: usize,
        entity_types: &mc_data::entity_types::EntityTypeRegistry,
        entity_loot: &mc_data::loot::entity_26_1_2::EntityLootCatalog,
    ) -> Result<MobBlueprint, String> {
        self.check(ordinal, entity_types, entity_loot)
            .map_err(|error| format!("mob blueprint {}: {error}", self.id))?;
        let wire = entity_types.id_of(&self.presents_as).expect("checked");
        let facts = entity_types.facts_of(&self.presents_as).expect("checked");
        self.goals.sort_by_key(|goal| goal.priority);
        Ok(MobBlueprint {
            name: self.id.as_str().to_owned(),
            type_id: CUSTOM_TYPE_FLAG | ((ordinal as u32) << 16) | wire,
            movement: self.movement,
            goals: self.goals,
            sensors: Sensors {
                sight_range: self.sight_range,
                attack_reach: self.attack_reach,
            },
            params: MobParams {
                max_health: self.max_health,
                speed: self.speed,
                attack_damage: self.attack_damage,
                pickup_delay: 0,
                breed_cooldown: 0,
                armor: self.armor,
                knockback_resistance: self.knockback_resistance,
            },
            half_width: facts.dimensions.half_width(),
            height: facts.dimensions.height,
            offspring: None,
            breed_food: Vec::new(),
            loot_table: self.loot_table,
            category: category(&self.category).expect("checked"),
            wool: false,
            spawn_weight: 0,
            capabilities: self.capabilities,
        })
    }

    fn check(
        &self,
        ordinal: usize,
        entity_types: &mc_data::entity_types::EntityTypeRegistry,
        entity_loot: &mc_data::loot::entity_26_1_2::EntityLootCatalog,
    ) -> Result<(), String> {
        if ordinal >= limits::MAX_CUSTOM_BLUEPRINTS {
            return Err(format!(
                "more than {} data-defined blueprints",
                limits::MAX_CUSTOM_BLUEPRINTS
            ));
        }
        if self.id.namespace() == "minecraft" {
            return Err("the minecraft namespace is reserved".into());
        }
        if self.id.as_str().len() > mc_script::MAX_SCRIPT_RESOURCE_ID_BYTES {
            return Err("id too long".into());
        }
        let wire = entity_types
            .id_of(&self.presents_as)
            .filter(|wire| *wire <= 0xFFFF)
            .ok_or_else(|| format!("unknown presents_as {}", self.presents_as))?;
        if REFUSED_PRESENTATIONS.contains(&self.presents_as.as_str()) {
            return Err(format!(
                "presents_as {} needs reserved multipart wire ids",
                self.presents_as
            ));
        }
        let living = mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(wire)
            .is_some_and(|contract| {
                contract.behavior.archetype == mc_data::entity_types::EntityArchetype::Living
                    && contract.name != "minecraft:player"
            });
        let Some(facts) = entity_types.facts_of(&self.presents_as).filter(|_| living) else {
            return Err(format!(
                "presents_as {} is not a vanilla living mob",
                self.presents_as
            ));
        };
        bounded(
            "presents_as body half_width",
            facts.dimensions.half_width(),
            &limits::BODY_HALF_WIDTH,
        )?;
        bounded(
            "presents_as body height",
            facts.dimensions.height,
            &limits::BODY_HEIGHT,
        )?;
        category(&self.category).ok_or_else(|| format!("unknown category {}", self.category))?;
        if !matches!(
            self.movement,
            MovementClass::Walker
                | MovementClass::Swimmer
                | MovementClass::Flyer
                | MovementClass::Amphibious
                | MovementClass::Stationary
        ) {
            return Err(format!(
                "movement {:?} is not a mob movement",
                self.movement
            ));
        }
        bounded("max_health", self.max_health, &limits::MAX_HEALTH)?;
        bounded("speed", self.speed, &limits::SPEED)?;
        bounded("attack_damage", self.attack_damage, &limits::ATTACK_DAMAGE)?;
        bounded("sight_range", self.sight_range, &limits::SIGHT_RANGE)?;
        bounded("attack_reach", self.attack_reach, &limits::ATTACK_REACH)?;
        if self.attack_reach > self.sight_range {
            return Err("attack_reach exceeds sight_range".into());
        }
        bounded("armor", self.armor, &limits::ARMOR)?;
        bounded(
            "knockback_resistance",
            self.knockback_resistance,
            &limits::KNOCKBACK_RESISTANCE,
        )?;
        if self.goals.is_empty() || self.goals.len() > limits::MAX_GOALS {
            return Err(format!("1..={} goals required", limits::MAX_GOALS));
        }
        for (index, goal) in self.goals.iter().enumerate() {
            if !DATA_GOALS.contains(&goal.kind) {
                return Err(format!("goal {:?} needs per-type state", goal.kind));
            }
            bounded(
                "goal priority",
                goal.priority,
                &(0..=limits::MAX_GOAL_PRIORITY),
            )?;
            if self.goals[..index]
                .iter()
                .any(|seen| seen.kind == goal.kind)
            {
                return Err(format!("duplicate goal {:?}", goal.kind));
            }
        }
        let has = |kind| self.goals.iter().any(|goal| goal.kind == kind);
        if has(GoalKind::Swell) != self.capabilities.explodes.is_some() {
            return Err("the swell goal and the explodes capability go together".into());
        }
        if has(GoalKind::Swell) && has(GoalKind::RangedAttack) {
            return Err("swell and ranged_attack are exclusive".into());
        }
        if let Some(explosion) = self.capabilities.explodes {
            bounded("explosion power", explosion.power, &limits::EXPLOSION_POWER)?;
            bounded("fuse_ticks", explosion.fuse_ticks, &limits::FUSE_TICKS)?;
        }
        if let Some(table) = &self.loot_table
            && !entity_loot.has_root(table)
        {
            return Err(format!("unknown loot table {table}"));
        }
        Ok(())
    }
}

fn category(name: &str) -> Option<MobCategory> {
    Some(match name {
        "monster" => MobCategory::Monster,
        "creature" => MobCategory::Creature,
        "ambient" => MobCategory::Ambient,
        "axolotls" => MobCategory::Axolotls,
        "underground_water_creature" => MobCategory::UndergroundWaterCreature,
        "water_creature" => MobCategory::WaterCreature,
        "water_ambient" => MobCategory::WaterAmbient,
        "misc" => MobCategory::Misc,
        _ => return None,
    })
}

/// Sheep wool entity data index (color id, `0x10` = sheared); the edge's
/// `mc_protocol` `SHEEP_ENTITY_DATA_WOOL_INDEX` is the wire evidence.
pub const SHEEP_WOOL_DATA_INDEX: u8 = 18;

/// Dye colors by id, as the loot tables name them (`minecraft:sheep/color`).
pub const DYE_COLORS: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

/// Vanilla `Sheep.getRandomSheepColor` from two uniform rolls: 5% each
/// black, gray and light gray, 3% brown, pink 1 in 500 of the rest, else
/// white.
#[must_use]
pub fn natural_wool_color(percent: u64, pink_roll: u64) -> u8 {
    match percent % 100 {
        0..5 => 15,
        5..10 => 7,
        10..15 => 8,
        15..18 => 12,
        _ if pink_roll.is_multiple_of(500) => 6,
        _ => 0,
    }
}

impl MobBlueprint {
    /// Whether this blueprint schedules a goal kind.
    #[must_use]
    pub fn has_goal(&self, kind: GoalKind) -> bool {
        self.goals.iter().any(|goal| goal.kind == kind)
    }
}

const ZOMBIE_GOALS: &[Goal] = &[
    Goal {
        priority: 0,
        kind: GoalKind::FleeFire,
    },
    Goal {
        priority: 1,
        kind: GoalKind::MeleeAttack,
    },
    Goal {
        priority: 2,
        kind: GoalKind::MeleeChase,
    },
    Goal {
        priority: 3,
        kind: GoalKind::Wander,
    },
];

/// Skeleton: shoots from a distance, otherwise idles about.
const SKELETON_GOALS: &[Goal] = &[
    Goal {
        priority: 0,
        kind: GoalKind::FleeFire,
    },
    Goal {
        priority: 1,
        kind: GoalKind::RangedAttack,
    },
    Goal {
        priority: 3,
        kind: GoalKind::Wander,
    },
];

/// Creeper: closes in (no melee damage) and swells next to its target.
const CREEPER_GOALS: &[Goal] = &[
    Goal {
        priority: 0,
        kind: GoalKind::FleeFire,
    },
    Goal {
        priority: 1,
        kind: GoalKind::Swell,
    },
    Goal {
        priority: 2,
        kind: GoalKind::MeleeChase,
    },
    Goal {
        priority: 3,
        kind: GoalKind::Wander,
    },
];

const ARROW_GOALS: &[Goal] = &[Goal {
    priority: 0,
    kind: GoalKind::Projectile,
}];

const ANIMAL_GOALS: &[Goal] = &[
    Goal {
        priority: 0,
        kind: GoalKind::FleeFire,
    },
    Goal {
        priority: 1,
        kind: GoalKind::Breed,
    },
    Goal {
        priority: 2,
        kind: GoalKind::Wander,
    },
    Goal {
        priority: 3,
        kind: GoalKind::Swim,
    },
];

const IDLE_GOALS: &[Goal] = &[Goal {
    priority: 0,
    kind: GoalKind::Idle,
}];

/// Vanilla blueprints the core simulates. No type-name branches in the tick:
/// the entity phase only reads these fields.
pub fn vanilla_blueprints(
    entity_types: &mc_data::entity_types::EntityTypeRegistry,
    items: &mc_data::items::ItemRegistry,
    entity_loot: &mc_data::loot::entity_26_1_2::EntityLootCatalog,
) -> Vec<(u32, MobBlueprint)> {
    let mut out = Vec::new();
    let mut push = |name: &'static str,
                    category: MobCategory,
                    movement: MovementClass,
                    goals: &'static [Goal],
                    sensors: Sensors,
                    params: MobParams,
                    half_width: f64,
                    height: f64,
                    offspring: Option<&'static str>,
                    breed_food: &[&str],
                    wool: bool| {
        let Ok(id) = mc_data::Identifier::parse(name) else {
            return;
        };
        let offspring = offspring.and_then(|offspring| {
            mc_data::Identifier::parse(offspring)
                .ok()
                .and_then(|id| entity_types.id_of(&id))
        });
        let breed_food = breed_food
            .iter()
            .filter_map(|food| {
                mc_data::Identifier::parse(*food)
                    .ok()
                    .and_then(|food| items.id_of(&food))
            })
            .collect();
        // Entities without independently sourced facts (iron golem, zombie villager)
        // still own the vanilla `entities/<name>` table when the catalog ships one.
        let loot_table = entity_types
            .facts_of(&id)
            .and_then(|facts| facts.loot_table.clone())
            .or_else(|| {
                mc_data::Identifier::parse(format!("minecraft:entities/{}", id.path()))
                    .ok()
                    .filter(|table| entity_loot.has_root(table))
            });
        if let Some(type_id) = entity_types.id_of(&id) {
            out.push((
                type_id,
                MobBlueprint {
                    name: name.to_owned(),
                    type_id,
                    movement,
                    goals: goals.to_vec(),
                    sensors,
                    params,
                    half_width,
                    height,
                    offspring,
                    breed_food,
                    loot_table,
                    category,
                    wool,
                    spawn_weight: 1,
                    capabilities: Capabilities::default(),
                },
            ));
        }
    };
    push(
        "minecraft:villager",
        MobCategory::Misc,
        MovementClass::Walker,
        IDLE_GOALS,
        Sensors {
            sight_range: 16.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 20.0,
            speed: 5.0,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 6000,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.95,
        None,
        &[],
        false,
    );
    push(
        "minecraft:zombie_villager",
        MobCategory::Monster,
        MovementClass::Walker,
        ZOMBIE_GOALS,
        Sensors {
            sight_range: 35.0,
            attack_reach: 2.2,
        },
        MobParams {
            max_health: 20.0,
            speed: 4.3,
            attack_damage: 3.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.95,
        None,
        &[],
        false,
    );
    push(
        "minecraft:iron_golem",
        MobCategory::Misc,
        MovementClass::Walker,
        ZOMBIE_GOALS,
        Sensors {
            sight_range: 16.0,
            attack_reach: 3.0,
        },
        MobParams {
            max_health: 100.0,
            speed: 2.5,
            attack_damage: 15.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.7,
        2.7,
        None,
        &[],
        false,
    );
    push(
        "minecraft:zombie",
        MobCategory::Monster,
        MovementClass::Walker,
        ZOMBIE_GOALS,
        Sensors {
            sight_range: 35.0,
            attack_reach: 2.2,
        },
        MobParams {
            max_health: 20.0,
            speed: 4.3,
            attack_damage: 3.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.95,
        None,
        &[],
        false,
    );
    // The spider melees like a zombie (faster, 2 damage); its night-only
    // hostility is [`Capabilities::night_aggro_only`].
    push(
        "minecraft:spider",
        MobCategory::Monster,
        MovementClass::Walker,
        ZOMBIE_GOALS,
        Sensors {
            sight_range: 16.0,
            attack_reach: 2.6,
        },
        MobParams {
            max_health: 16.0,
            speed: 5.6,
            attack_damage: 2.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.7,
        0.9,
        None,
        &[],
        false,
    );
    // The skeleton's `attack_damage` is its arrow base damage and
    // `attack_reach` its shooting range (vanilla `RangedBowAttackGoal` 15).
    push(
        "minecraft:skeleton",
        MobCategory::Monster,
        MovementClass::Walker,
        SKELETON_GOALS,
        Sensors {
            sight_range: 16.0,
            attack_reach: 15.0,
        },
        MobParams {
            max_health: 20.0,
            speed: 4.7,
            attack_damage: 2.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.99,
        None,
        &[],
        false,
    );
    // The creeper's `attack_reach` is the distance that starts its fuse.
    push(
        "minecraft:creeper",
        MobCategory::Monster,
        MovementClass::Walker,
        CREEPER_GOALS,
        Sensors {
            sight_range: 16.0,
            attack_reach: 3.0,
        },
        MobParams {
            max_health: 20.0,
            speed: 4.7,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.7,
        None,
        &[],
        false,
    );
    push(
        "minecraft:arrow",
        MobCategory::Misc,
        MovementClass::Projectile,
        ARROW_GOALS,
        Sensors {
            sight_range: 0.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 1.0,
            speed: 0.0,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.25,
        0.5,
        None,
        &[],
        false,
    );
    push(
        "minecraft:cow",
        MobCategory::Creature,
        MovementClass::Walker,
        ANIMAL_GOALS,
        Sensors {
            sight_range: 10.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 10.0,
            speed: 2.0,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 6000,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.45,
        1.4,
        Some("minecraft:cow"),
        &["minecraft:wheat"],
        false,
    );
    push(
        "minecraft:pig",
        MobCategory::Creature,
        MovementClass::Walker,
        ANIMAL_GOALS,
        Sensors {
            sight_range: 10.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 10.0,
            speed: 2.5,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 6000,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.45,
        0.9,
        Some("minecraft:pig"),
        &["minecraft:carrot", "minecraft:potato", "minecraft:beetroot"],
        false,
    );
    push(
        "minecraft:sheep",
        MobCategory::Creature,
        MovementClass::Walker,
        ANIMAL_GOALS,
        Sensors {
            sight_range: 10.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 8.0,
            speed: 2.3,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 6000,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.45,
        1.3,
        Some("minecraft:sheep"),
        &["minecraft:wheat"],
        true,
    );
    push(
        "minecraft:chicken",
        MobCategory::Creature,
        MovementClass::Walker,
        ANIMAL_GOALS,
        Sensors {
            sight_range: 10.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 4.0,
            speed: 2.5,
            attack_damage: 0.0,
            pickup_delay: 0,
            breed_cooldown: 6000,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.2,
        0.7,
        Some("minecraft:chicken"),
        &[
            "minecraft:wheat_seeds",
            "minecraft:melon_seeds",
            "minecraft:pumpkin_seeds",
            "minecraft:beetroot_seeds",
            "minecraft:torchflower_seeds",
            "minecraft:pitcher_pod",
        ],
        false,
    );
    push(
        "minecraft:item",
        MobCategory::Misc,
        MovementClass::Item,
        IDLE_GOALS,
        Sensors {
            sight_range: 0.0,
            attack_reach: 0.0,
        },
        MobParams {
            max_health: 1.0,
            speed: 0.0,
            attack_damage: 0.0,
            pickup_delay: 10,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.125,
        0.25,
        None,
        &[],
        false,
    );
    push(
        "minecraft:player",
        MobCategory::Misc,
        MovementClass::Player,
        IDLE_GOALS,
        Sensors {
            sight_range: 0.0,
            attack_reach: 3.0,
        },
        MobParams {
            max_health: 20.0,
            speed: 4.3,
            attack_damage: 1.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        0.3,
        1.8,
        None,
        &[],
        false,
    );
    // Natural spawn weights (vanilla `SpawnerData`).
    for (_, blueprint) in &mut out {
        blueprint.spawn_weight = match blueprint.name.as_str() {
            "minecraft:zombie"
            | "minecraft:zombie_villager"
            | "minecraft:skeleton"
            | "minecraft:spider"
            | "minecraft:creeper" => 100,
            "minecraft:sheep" => 12,
            "minecraft:pig" | "minecraft:chicken" => 10,
            "minecraft:cow" => 8,
            _ => 1,
        };
    }
    crate::special_mobs::extend_blueprints(entity_types, entity_loot, &mut out);
    crate::copper_golem::extend_blueprints(entity_types, &mut out);
    crate::end_fight::extend_blueprints(entity_types, &mut out);
    crate::species::extend_blueprints(entity_types, items, entity_loot, &mut out);
    crate::projectiles::extend_blueprints(entity_types, &mut out);
    crate::vehicles::extend_blueprints(entity_types, &mut out);
    for (_, blueprint) in &mut out {
        vanilla_capabilities(blueprint);
    }
    let id = |name: &str| {
        entity_types.id_of(&mc_data::Identifier::parse(name).expect("canonical entity name"))
    };
    if let (Some(horse), Some(donkey), Some(mule)) = (
        id("minecraft:horse"),
        id("minecraft:donkey"),
        id("minecraft:mule"),
    ) {
        for (key, blueprint) in &mut out {
            let partner = if *key == horse {
                Some(donkey)
            } else if *key == donkey {
                Some(horse)
            } else {
                None
            };
            blueprint.capabilities.crossbreed = partner.map(|partner| Crossbreed {
                partner,
                offspring: mule,
            });
        }
    }
    out
}

/// `#minecraft:fall_damage_immune` of the 26.1.2 data pack.
const FALL_DAMAGE_IMMUNE: [&str; 18] = [
    "minecraft:copper_golem",
    "minecraft:iron_golem",
    "minecraft:snow_golem",
    "minecraft:shulker",
    "minecraft:allay",
    "minecraft:bat",
    "minecraft:bee",
    "minecraft:blaze",
    "minecraft:cat",
    "minecraft:chicken",
    "minecraft:ghast",
    "minecraft:happy_ghast",
    "minecraft:phantom",
    "minecraft:magma_cube",
    "minecraft:ocelot",
    "minecraft:parrot",
    "minecraft:wither",
    "minecraft:breeze",
];

/// Vanilla capability data: the only place the shared-behaviour type lists
/// live. The tick reads [`MobBlueprint::capabilities`] and params, never names.
fn vanilla_capabilities(blueprint: &mut MobBlueprint) {
    let name = blueprint.name.as_str();
    blueprint.capabilities = Capabilities {
        burns_in_daylight: matches!(
            name,
            "minecraft:zombie"
                | "minecraft:zombie_villager"
                | "minecraft:skeleton"
                | "minecraft:stray"
                | "minecraft:bogged"
                | "minecraft:parched"
                | "minecraft:phantom"
        ),
        night_aggro_only: matches!(name, "minecraft:spider" | "minecraft:cave_spider"),
        hurt_by_water: name == "minecraft:enderman",
        fall_damage_immune: FALL_DAMAGE_IMMUNE.contains(&name),
        climbs_walls: matches!(name, "minecraft:spider" | "minecraft:cave_spider"),
        inverted_healing: matches!(
            name,
            "minecraft:zombie"
                | "minecraft:zombie_villager"
                | "minecraft:husk"
                | "minecraft:drowned"
                | "minecraft:zombified_piglin"
                | "minecraft:zoglin"
                | "minecraft:skeleton"
                | "minecraft:stray"
                | "minecraft:bogged"
                | "minecraft:wither_skeleton"
                | "minecraft:wither"
                | "minecraft:phantom"
                | "minecraft:zombie_horse"
                | "minecraft:skeleton_horse"
        ),
        aquatic: match name {
            "minecraft:cod"
            | "minecraft:salmon"
            | "minecraft:tropical_fish"
            | "minecraft:pufferfish"
            | "minecraft:tadpole" => AquaticControl::Fish,
            "minecraft:squid" | "minecraft:glow_squid" => AquaticControl::Squid,
            "minecraft:dolphin" => AquaticControl::Dolphin,
            _ => AquaticControl::Other,
        },
        explodes: (name == "minecraft:creeper").then_some(Explosion {
            power: crate::hostile::CREEPER_POWER,
            fuse_ticks: crate::hostile::FUSE_TICKS,
            griefing: true,
        }),
        temptable: blueprint.has_goal(GoalKind::Breed) && !blueprint.breed_food.is_empty(),
        follows_adults: blueprint.has_goal(GoalKind::Breed) && blueprint.offspring.is_some(),
        crossbreed: None,
    };
    blueprint.params.armor = match name {
        "minecraft:zombie"
        | "minecraft:zombie_villager"
        | "minecraft:husk"
        | "minecraft:drowned"
        | "minecraft:zombified_piglin" => 2.0,
        _ => 0.0,
    };
    blueprint.params.knockback_resistance = match name {
        "minecraft:iron_golem" => 1.0,
        "minecraft:hoglin" | "minecraft:zoglin" => 0.6,
        "minecraft:ravager" => 0.75,
        _ => 0.0,
    };
}
