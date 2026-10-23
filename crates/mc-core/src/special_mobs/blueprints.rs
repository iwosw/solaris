use crate::blueprint::{Goal, GoalKind, MobBlueprint, MobParams, MovementClass, Sensors};

pub(crate) fn extend_blueprints(
    types: &mc_data::entity_types::EntityTypeRegistry,
    loot: &mc_data::loot::entity_26_1_2::EntityLootCatalog,
    out: &mut Vec<(u32, MobBlueprint)>,
) {
    use GoalKind::*;
    use MovementClass::*;
    // Dedicated goals are registered before the generic species front door.
    const SPECS: &[(&str, GoalKind, MovementClass, f32, f32, f32)] = &[
        ("minecraft:blaze", BlazeBurst, Flyer, 20.0, 48.0, 6.0),
        ("minecraft:ghast", SiegeShot, Flyer, 10.0, 64.0, 0.0),
        ("minecraft:pillager", Crossbow, Walker, 24.0, 8.0, 2.0),
        ("minecraft:witch", Alchemist, Walker, 26.0, 10.0, 0.0),
        ("minecraft:breeze", WindCharge, Walker, 30.0, 16.0, 0.0),
        ("minecraft:guardian", Beam, Swimmer, 30.0, 16.0, 6.0),
        (
            "minecraft:elder_guardian",
            ElderBeam,
            Swimmer,
            80.0,
            16.0,
            8.0,
        ),
        ("minecraft:warden", SonicBoom, Walker, 500.0, 24.0, 30.0),
        ("minecraft:evoker", Fangs, Walker, 24.0, 12.0, 0.0),
        ("minecraft:shulker", HomingShot, Stationary, 30.0, 20.0, 0.0),
        ("minecraft:wither", WitherBarrage, Flyer, 300.0, 20.0, 0.0),
        (
            "minecraft:evoker_fangs",
            FangLife,
            Stationary,
            1.0,
            0.0,
            0.0,
        ),
        ("minecraft:vex", VexLife, Flyer, 14.0, 16.0, 4.0),
    ];
    const GOALS: &[(GoalKind, &[Goal])] = &[
        (
            BlazeBurst,
            &[Goal {
                priority: 0,
                kind: BlazeBurst,
            }],
        ),
        (
            SiegeShot,
            &[Goal {
                priority: 0,
                kind: SiegeShot,
            }],
        ),
        (
            Crossbow,
            &[Goal {
                priority: 0,
                kind: Crossbow,
            }],
        ),
        (
            Alchemist,
            &[Goal {
                priority: 0,
                kind: Alchemist,
            }],
        ),
        (
            WindCharge,
            &[Goal {
                priority: 0,
                kind: WindCharge,
            }],
        ),
        (
            Beam,
            &[Goal {
                priority: 0,
                kind: Beam,
            }],
        ),
        (
            ElderBeam,
            &[Goal {
                priority: 0,
                kind: ElderBeam,
            }],
        ),
        (
            SonicBoom,
            &[
                Goal {
                    priority: 0,
                    kind: SonicBoom,
                },
                Goal {
                    priority: 1,
                    kind: MeleeAttack,
                },
                Goal {
                    priority: 2,
                    kind: MeleeChase,
                },
            ],
        ),
        (
            Fangs,
            &[Goal {
                priority: 0,
                kind: Fangs,
            }],
        ),
        (
            HomingShot,
            &[Goal {
                priority: 0,
                kind: HomingShot,
            }],
        ),
        (
            WitherBarrage,
            &[Goal {
                priority: 0,
                kind: WitherBarrage,
            }],
        ),
        (
            FangLife,
            &[Goal {
                priority: 0,
                kind: FangLife,
            }],
        ),
        (
            VexLife,
            &[
                Goal {
                    priority: 0,
                    kind: VexLife,
                },
                Goal {
                    priority: 1,
                    kind: MeleeAttack,
                },
                Goal {
                    priority: 2,
                    kind: MeleeChase,
                },
            ],
        ),
    ];
    for &(name, kind, movement, health, range, damage) in SPECS {
        let id = mc_data::Identifier::parse(name).expect("canonical special mob");
        let Some(type_id) = types.id_of(&id) else {
            continue;
        };
        if out.iter().any(|(id, _)| *id == type_id) {
            continue;
        }
        let Some(facts) = types.facts_of(&id) else {
            continue;
        };
        let table = mc_data::Identifier::parse(format!("minecraft:entities/{}", id.path()))
            .expect("canonical loot");
        out.push((
            type_id,
            MobBlueprint {
                name: name.to_owned(),
                type_id,
                movement,
                goals: GOALS
                    .iter()
                    .find(|(goal, _)| *goal == kind)
                    .expect("special goal")
                    .1
                    .to_vec(),
                sensors: Sensors {
                    sight_range: range,
                    attack_reach: if matches!(kind, SonicBoom | VexLife) {
                        2.0
                    } else {
                        range
                    },
                },
                params: MobParams {
                    max_health: health,
                    speed: if matches!(movement, Stationary) {
                        0.0
                    } else {
                        facts.attributes.movement_speed.unwrap_or(0.25) as f32 * 10.0
                    },
                    attack_damage: damage,
                    pickup_delay: 0,
                    breed_cooldown: 0,
                    armor: 0.0,
                    knockback_resistance: 0.0,
                },
                half_width: facts.dimensions.half_width(),
                height: facts.dimensions.height,
                offspring: None,
                breed_food: Vec::new(),
                loot_table: loot.has_root(&table).then_some(table),
                category: mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(
                    type_id,
                )
                .expect("contract")
                .mob_category(),
                wool: false,
                spawn_weight: 1,
                capabilities: crate::blueprint::Capabilities::default(),
            },
        ));
    }
}
