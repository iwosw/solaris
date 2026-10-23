//! Living species baselines composed from the 26.1.2 data contracts.
//!
//! Registry dimensions/categories and common movement/combat families are
//! authoritative data. The small supplemental parameter table supplies gameplay
//! tunables for species whose scalar attributes are not yet in mc-data.

use crate::blueprint::{Goal, GoalKind, MobBlueprint, MobParams, MovementClass, Sensors};
use mc_data::entity_types::{
    EntityArchetype, EntityTypeRegistry, entity_type_contract_26_1_2_by_protocol_id,
};
use mc_data::mob_behavior_26_1_2::{MobBehaviorTable, MobCombatPolicy, MobMovementPolicy};

const PASSIVE: &[Goal] = &[
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
const MELEE: &[Goal] = &[
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
const MELEE_BREED: &[Goal] = &[
    Goal {
        priority: 0,
        kind: GoalKind::Breed,
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
const RANGED: &[Goal] = &[
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
const WANDER: &[Goal] = &[Goal {
    priority: 0,
    kind: GoalKind::Wander,
}];
const STATIONARY: &[Goal] = &[Goal {
    priority: 0,
    kind: GoalKind::Idle,
}];

/// Append supported baselines without replacing a slice's custom blueprint.
/// Special ranged/beam/boss families must register their own native goals; they
/// are deliberately not silently converted into generic melee mobs.
pub fn extend_blueprints(
    entity_types: &EntityTypeRegistry,
    items: &mc_data::items::ItemRegistry,
    loot: &mc_data::loot::entity_26_1_2::EntityLootCatalog,
    out: &mut Vec<(u32, MobBlueprint)>,
) {
    let behavior = MobBehaviorTable::vanilla_26_1_2();
    for type_id in 0..mc_data::entity_types::ENTITY_TYPE_COUNT as u32 {
        let Some(contract) = entity_type_contract_26_1_2_by_protocol_id(type_id) else {
            continue;
        };
        if contract.behavior.archetype != EntityArchetype::Living
            || contract.name == "minecraft:player"
            || out.iter().any(|(id, _)| *id == type_id)
        {
            continue;
        }
        let Some(profile) = behavior.get_by_name(contract.name) else {
            continue;
        };
        if !matches!(
            profile.combat,
            MobCombatPolicy::None
                | MobCombatPolicy::Melee
                | MobCombatPolicy::Arrow
                | MobCombatPolicy::Snowball
        ) {
            continue;
        }
        let id = mc_data::Identifier::parse(contract.name).expect("canonical entity name");
        let Some(facts) = entity_types.facts_of(&id) else {
            continue;
        };
        let movement = match profile.movement {
            MobMovementPolicy::Immobile => MovementClass::Stationary,
            MobMovementPolicy::FlyingWander => MovementClass::Flyer,
            MobMovementPolicy::AquaticWander => MovementClass::Swimmer,
            MobMovementPolicy::AmphibiousWander => MovementClass::Amphibious,
            _ => MovementClass::Walker,
        };
        let foods = breeding_food(contract.name);
        let breed_food: Vec<u32> = foods
            .iter()
            .filter_map(|name| {
                items.id_of(&mc_data::Identifier::parse(*name).expect("canonical food"))
            })
            .collect();
        let offspring = (!breed_food.is_empty()).then_some(type_id);
        let supplemental = parameters(contract.name);
        let hostile = profile.combat == MobCombatPolicy::Melee;
        let speed = facts
            .attributes
            .movement_speed
            .map(|value| value * 10.0)
            .unwrap_or(supplemental.1);
        let table = mc_data::Identifier::parse(format!("minecraft:entities/{}", id.path()))
            .expect("canonical loot table");
        let loot_table = loot.has_root(&table).then_some(table);
        // Giants retain ground physics and attributes, but vanilla registers
        // no AI goals (including no wandering).
        let goals: &[Goal] = if contract.name == "minecraft:giant" {
            &[]
        } else if movement == MovementClass::Stationary {
            STATIONARY
        } else if profile.combat == MobCombatPolicy::Arrow {
            RANGED
        } else if hostile && offspring.is_some() {
            MELEE_BREED
        } else if hostile {
            MELEE
        } else if offspring.is_some() {
            PASSIVE
        } else {
            WANDER
        };
        out.push((
            type_id,
            MobBlueprint {
                name: contract.name.to_owned(),
                type_id,
                movement,
                goals: goals.to_vec(),
                sensors: Sensors {
                    sight_range: facts.attributes.follow_range.unwrap_or(16.0) as f32,
                    attack_reach: if profile.combat == MobCombatPolicy::Arrow {
                        15.0
                    } else if hostile {
                        (facts.dimensions.width + 1.2) as f32
                    } else {
                        0.0
                    },
                },
                params: MobParams {
                    // SnowGolem.createAttributes in the local 26.1.2 jar: four health points.
                    max_health: if profile.combat == MobCombatPolicy::Snowball {
                        4.0
                    } else {
                        facts.attributes.max_health.unwrap_or(supplemental.0) as f32
                    },
                    speed: speed as f32,
                    attack_damage: facts.attributes.attack_damage.unwrap_or(
                        if hostile || profile.combat == MobCombatPolicy::Arrow {
                            supplemental.2
                        } else {
                            0.0
                        },
                    ) as f32,
                    pickup_delay: 0,
                    breed_cooldown: if offspring.is_some() { 6000 } else { 0 },
                    armor: 0.0,
                    knockback_resistance: 0.0,
                },
                half_width: facts.dimensions.half_width(),
                height: facts.dimensions.height,
                offspring,
                breed_food,
                loot_table,
                category: contract.mob_category(),
                wool: false,
                spawn_weight: 1,
                capabilities: crate::blueprint::Capabilities::default(),
            },
        ));
    }
}

/// Swimmer steering family, a blueprint capability.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AquaticControl {
    Fish,
    Squid,
    Dolphin,
    #[default]
    Other,
}

/// Resolve species food tags once at startup; the loaded data pack's membership
/// is used rather than a second hard-coded per-tick item policy.
pub(crate) fn apply_food_tags(
    tags: &mc_data::tags::TagsData,
    blueprints: &mut [(u32, MobBlueprint)],
) {
    let registry = mc_data::Identifier::parse("minecraft:item").expect("canonical registry");
    let Some(item_tags) = tags.registries.get(&registry) else {
        return;
    };
    for (_, blueprint) in blueprints {
        if blueprint.offspring.is_none() {
            continue;
        }
        let species = blueprint
            .name
            .strip_prefix("minecraft:")
            .expect("vanilla namespace");
        // Horse/llama tags include healing and growth food that is not breeding
        // food; their golden-food/hay prerequisites are deliberately retained.
        if matches!(species, "horse" | "donkey" | "llama" | "trader_llama") {
            continue;
        }
        let tag_species = if species == "mooshroom" {
            "cow"
        } else {
            species
        };
        let tag = mc_data::Identifier::parse(format!("minecraft:{tag_species}_food"))
            .expect("canonical food tag");
        if let Some(ids) = item_tags.get(&tag) {
            blueprint.breed_food = ids
                .iter()
                .filter_map(|id| u32::try_from(*id).ok())
                .collect();
        }
    }
}

/// Approximate vanilla gameplay parameters where mc-data has no scalar row:
/// health, movement in blocks/second, base melee damage. Protocol/layout facts
/// never use these tunables.
fn parameters(name: &str) -> (f64, f64, f64) {
    match name {
        "minecraft:rabbit" => (3.0, 3.0, 0.0),
        "minecraft:mooshroom" => (10.0, 2.0, 0.0),
        "minecraft:fox" | "minecraft:ocelot" | "minecraft:cat" => (10.0, 3.0, 2.0),
        "minecraft:panda" => (20.0, 1.5, 6.0),
        "minecraft:polar_bear" => (30.0, 2.5, 6.0),
        "minecraft:goat" => (10.0, 2.0, 2.0),
        "minecraft:armadillo" => (12.0, 1.4, 0.0),
        "minecraft:sniffer" => (14.0, 1.0, 0.0),
        "minecraft:horse"
        | "minecraft:donkey"
        | "minecraft:mule"
        | "minecraft:skeleton_horse"
        | "minecraft:zombie_horse" => (15.0, 2.25, 3.0),
        "minecraft:llama" | "minecraft:trader_llama" => (15.0, 1.75, 1.0),
        "minecraft:camel" | "minecraft:camel_husk" => (32.0, 3.0, 3.0),
        "minecraft:strider" => (20.0, 1.75, 0.0),
        "minecraft:wolf" => (8.0, 3.0, 4.0),
        "minecraft:bat" => (6.0, 2.0, 0.0),
        "minecraft:bee" => (10.0, 3.0, 2.0),
        "minecraft:parrot" => (6.0, 2.0, 0.0),
        "minecraft:allay" => (20.0, 2.0, 0.0),
        "minecraft:happy_ghast" => (20.0, 1.0, 0.0),
        "minecraft:pufferfish" | "minecraft:tadpole" => (3.0, 0.8, 0.0),
        "minecraft:squid" | "minecraft:glow_squid" => (10.0, 0.8, 0.0),
        "minecraft:dolphin" => (10.0, 1.2, 3.0),
        "minecraft:nautilus" | "minecraft:zombie_nautilus" => (15.0, 0.8, 3.0),
        "minecraft:axolotl" => (14.0, 0.8, 2.0),
        "minecraft:frog" => (10.0, 1.0, 0.0),
        "minecraft:turtle" => (30.0, 0.8, 0.0),
        "minecraft:husk" | "minecraft:drowned" | "minecraft:zombified_piglin" => (20.0, 2.3, 3.0),
        "minecraft:stray" | "minecraft:parched" => (20.0, 2.5, 2.0),
        "minecraft:wither_skeleton" => (20.0, 2.5, 4.0),
        "minecraft:bogged" => (16.0, 2.5, 2.0),
        "minecraft:cave_spider" => (12.0, 3.0, 2.0),
        "minecraft:silverfish" | "minecraft:endermite" => (8.0, 2.5, 2.0),
        "minecraft:enderman" => (40.0, 3.0, 7.0),
        "minecraft:slime" | "minecraft:magma_cube" => (4.0, 2.0, 2.0),
        "minecraft:hoglin" | "minecraft:zoglin" => (40.0, 3.0, 6.0),
        "minecraft:piglin" => (16.0, 3.5, 5.0),
        "minecraft:piglin_brute" => (50.0, 3.5, 7.0),
        "minecraft:vindicator" => (24.0, 3.5, 5.0),
        "minecraft:ravager" => (100.0, 3.0, 12.0),
        "minecraft:giant" => (100.0, 5.0, 50.0),
        "minecraft:illusioner" => (32.0, 3.5, 5.0),
        "minecraft:vex" => (14.0, 4.0, 4.0),
        "minecraft:creaking" => (1.0, 3.0, 3.0),
        "minecraft:phantom" => (20.0, 4.0, 6.0),
        "minecraft:copper_golem" => (12.0, 2.0, 0.0),
        "minecraft:armor_stand" | "minecraft:mannequin" => (20.0, 0.0, 0.0),
        _ => (20.0, 2.5, 3.0),
    }
}

pub(crate) fn breeding_food(name: &str) -> &'static [&'static str] {
    match name {
        "minecraft:cow" | "minecraft:mooshroom" | "minecraft:sheep" | "minecraft:goat" => {
            &["minecraft:wheat"]
        }
        "minecraft:pig" => &["minecraft:carrot", "minecraft:potato", "minecraft:beetroot"],
        "minecraft:rabbit" => &[
            "minecraft:carrot",
            "minecraft:golden_carrot",
            "minecraft:dandelion",
        ],
        "minecraft:fox" => &["minecraft:sweet_berries", "minecraft:glow_berries"],
        "minecraft:ocelot" | "minecraft:cat" => &["minecraft:cod", "minecraft:salmon"],
        "minecraft:panda" => &["minecraft:bamboo"],
        "minecraft:armadillo" => &["minecraft:spider_eye"],
        "minecraft:sniffer" => &["minecraft:torchflower_seeds"],
        "minecraft:horse" | "minecraft:donkey" => &[
            "minecraft:golden_carrot",
            "minecraft:golden_apple",
            "minecraft:enchanted_golden_apple",
        ],
        "minecraft:llama" => &["minecraft:hay_block"],
        "minecraft:camel" => &["minecraft:cactus"],
        "minecraft:strider" => &["minecraft:warped_fungus"],
        "minecraft:wolf" => &[
            "minecraft:beef",
            "minecraft:cooked_beef",
            "minecraft:porkchop",
            "minecraft:cooked_porkchop",
            "minecraft:mutton",
            "minecraft:cooked_mutton",
            "minecraft:chicken",
            "minecraft:cooked_chicken",
            "minecraft:rabbit",
            "minecraft:cooked_rabbit",
            "minecraft:rotten_flesh",
        ],
        "minecraft:bee" => &[
            "minecraft:dandelion",
            "minecraft:poppy",
            "minecraft:blue_orchid",
            "minecraft:allium",
            "minecraft:azure_bluet",
            "minecraft:oxeye_daisy",
            "minecraft:cornflower",
            "minecraft:lily_of_the_valley",
            "minecraft:sunflower",
            "minecraft:rose_bush",
            "minecraft:lilac",
            "minecraft:peony",
            "minecraft:torchflower",
            "minecraft:pitcher_plant",
            "minecraft:white_tulip",
            "minecraft:red_tulip",
            "minecraft:orange_tulip",
            "minecraft:pink_tulip",
        ],
        "minecraft:happy_ghast" => &["minecraft:snowball"],
        "minecraft:nautilus" => &["minecraft:pufferfish"],
        "minecraft:axolotl" => &["minecraft:tropical_fish_bucket"],
        "minecraft:frog" => &["minecraft:slime_ball"],
        "minecraft:turtle" => &["minecraft:seagrass"],
        "minecraft:hoglin" => &["minecraft:crimson_fungus"],
        _ => &[],
    }
}
