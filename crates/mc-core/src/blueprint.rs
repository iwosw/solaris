//! Mob blueprints as data (ADR 0013 "Mob blueprints").
//!
//! A mob type, vanilla or plugin, is a blueprint: components, prioritized
//! goals, sensors, movement/physics class, parameters. The entity phase
//! executes blueprints natively; there is no per-type code path a plugin
//! type cannot use.

/// How the entity phase moves this mob. Selects a [`mc_physics`]
/// [`PhysicsConfig`](mc_physics::PhysicsConfig), never a type-name branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Goal {
    /// Priority; evaluated in ascending order.
    pub priority: u8,
    /// What the goal does.
    pub kind: GoalKind,
}

/// Goal behaviours the entity phase executes natively.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    /// (vanilla creeper `SwellGoal`).
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
}

/// Vanilla categories, caps and despawn distances share the data contract.
pub use mc_data::entity_types::MobCategory;

/// One mob type as data.
#[derive(Debug, Clone, PartialEq)]
pub struct MobBlueprint {
    /// Namespaced id, e.g. `minecraft:zombie`.
    pub name: &'static str,
    /// Protocol entity type id (26.1.2 registry).
    pub type_id: u32,
    /// Movement/physics class.
    pub movement: MovementClass,
    /// Goals sorted by priority ascending.
    pub goals: &'static [Goal],
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
    /// Ignites under open sky by day (vanilla `isSunBurnTick` mobs).
    pub burns_in_daylight: bool,
    /// Hostile only at night: by day it ignores players (vanilla spider).
    pub night_aggro_only: bool,
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
                    name,
                    type_id,
                    movement,
                    goals,
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
                    burns_in_daylight: false,
                    night_aggro_only: false,
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
        },
        0.3,
        1.95,
        None,
        &[],
        false,
    );
    // The spider melees like a zombie (faster, 2 damage); its night-only
    // hostility is [`MobBlueprint::night_aggro_only`].
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
        },
        0.3,
        1.8,
        None,
        &[],
        false,
    );
    // Natural spawn weights (vanilla `SpawnerData`) and the sun/night rules.
    for (_, blueprint) in &mut out {
        let (weight, burns, night_only) = match blueprint.name {
            "minecraft:zombie" | "minecraft:zombie_villager" | "minecraft:skeleton" => {
                (100, true, false)
            }
            "minecraft:spider" => (100, false, true),
            "minecraft:creeper" => (100, false, false),
            "minecraft:sheep" => (12, false, false),
            "minecraft:pig" | "minecraft:chicken" => (10, false, false),
            "minecraft:cow" => (8, false, false),
            _ => (1, false, false),
        };
        blueprint.spawn_weight = weight;
        blueprint.burns_in_daylight = burns;
        blueprint.night_aggro_only = night_only;
    }
    crate::special_mobs::extend_blueprints(entity_types, entity_loot, &mut out);
    crate::copper_golem::extend_blueprints(entity_types, &mut out);
    crate::end_fight::extend_blueprints(entity_types, &mut out);
    crate::species::extend_blueprints(entity_types, items, entity_loot, &mut out);
    crate::projectiles::extend_blueprints(entity_types, &mut out);
    crate::vehicles::extend_blueprints(entity_types, &mut out);
    out
}
