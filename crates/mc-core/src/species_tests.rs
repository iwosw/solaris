use std::sync::Arc;

use crate::blueprint::{GoalKind, MovementClass};
use crate::boundary::{ChunkPos, EntityId};
use crate::entities::EntityKind;
use crate::intent::{Footprint, Intent, IntentKind};
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};

const BASELINES: &[&str] = &[
    "cow",
    "pig",
    "sheep",
    "chicken",
    "rabbit",
    "mooshroom",
    "fox",
    "ocelot",
    "panda",
    "polar_bear",
    "goat",
    "armadillo",
    "sniffer",
    "horse",
    "donkey",
    "mule",
    "llama",
    "trader_llama",
    "camel",
    "strider",
    "wolf",
    "cat",
    "bat",
    "bee",
    "parrot",
    "allay",
    "happy_ghast",
    "cod",
    "salmon",
    "tropical_fish",
    "pufferfish",
    "squid",
    "glow_squid",
    "dolphin",
    "nautilus",
    "axolotl",
    "frog",
    "turtle",
    "tadpole",
    "zombie",
    "spider",
    "husk",
    "drowned",
    "zombified_piglin",
    "zombie_horse",
    "zombie_nautilus",
    "camel_husk",
    "stray",
    "bogged",
    "parched",
    "wither_skeleton",
    "cave_spider",
    "silverfish",
    "endermite",
    "enderman",
    "slime",
    "magma_cube",
    "hoglin",
    "zoglin",
    "piglin",
    "piglin_brute",
    "vindicator",
    "ravager",
    "giant",
    "illusioner",
    "vex",
    "creaking",
    "phantom",
    "wandering_trader",
    "copper_golem",
    "armor_stand",
    "mannequin",
];

struct Habitat {
    air: u32,
    ground: u32,
    water: u32,
}

impl mc_world::ChunkGenerator for Habitat {
    fn generate(&self, pos: ChunkPos) -> mc_world::Chunk {
        let mut chunk = mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.air),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        );
        for x in 0..16 {
            for z in 0..16 {
                chunk.set_block(x, 64, z, mc_world::BlockStateId(self.ground));
                if pos.x >= 1 {
                    for y in 65..85 {
                        chunk.set_block(x, y, z, mc_world::BlockStateId(self.water));
                    }
                }
            }
        }
        chunk
    }
}

fn state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator = Arc::new(Habitat {
        air: registry.states.air,
        ground: registry.states.grass_block,
        water: registry.states.water,
    });
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 65, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 3, 512);
    state
        .tickets
        .insert("fixture".into(), (ChunkPos { x: 0, z: 0 }, 3));
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.day_time = 18000;
    state
}

fn type_id(registry: &CoreRegistry, name: &str) -> u32 {
    registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
}

#[test]
fn every_assigned_species_spawns_locomotes_and_dies_through_native_merge() {
    let mut state = state();
    for name in BASELINES {
        let type_id = type_id(&state.registry, name);
        let blueprint = state.registry.blueprint(type_id).expect(name).clone();
        let pos = match blueprint.movement {
            MovementClass::Swimmer | MovementClass::Amphibious => [24.5, 72.0, 0.5],
            MovementClass::Flyer => [0.5, 72.0, 0.5],
            _ => [0.5, 65.0, 0.5],
        };
        let id = state.spawn_mob(type_id, pos, blueprint.params.max_health);
        let slot = state.entities.slot_of(id).unwrap();
        assert_eq!(
            state.entities.body(slot).movement,
            blueprint.movement,
            "{name}"
        );
        state.entities.set_ground(slot, pos[1] == 65.0);
        // Id-shifted decisions may initially choose idle; cover the 80-tick
        // wander phase so a real missing movement dispatch cannot pass.
        for _ in 0..90 {
            run_tick(&mut state, &[]);
        }
        let slot = state
            .entities
            .slot_of(id)
            .unwrap_or_else(|| panic!("{name} died in suitable habitat"));
        let moved = state.entities.pos(slot);
        if blueprint.movement == MovementClass::Stationary || blueprint.goals.is_empty() {
            assert_eq!(moved, pos, "{name} must not wander without goals");
        } else {
            assert_ne!(moved, pos, "{name} movement family must execute");
        }
        let mut intents = vec![Intent {
            actor: EntityId(i32::MAX),
            seq: 0,
            kind: IntentKind::DamageEntity {
                target: id,
                amount: 1000.0,
                kind: crate::survival::DamageKind::Melee,
                source: None,
            },
            footprint: Footprint {
                entity_writes: vec![id],
                ..Default::default()
            },
        }];
        assert_eq!(state.merge(&mut intents), 1, "{name} hit admitted");
        let (deaths, _, spawned, _) = state.commit_merge_effects();
        assert!(deaths.contains(&id), "{name} lethal damage");
        assert!(
            state.entities.slot_of_any(id).is_none(),
            "{name} death tombstone"
        );
        for item in spawned {
            if let Some(item_slot) = state.entities.slot_of(item) {
                assert!(matches!(
                    state.entities.body(item_slot).kind,
                    EntityKind::Item { .. }
                        | EntityKind::ExperienceOrb { .. }
                        | EntityKind::Mob { .. }
                ));
            }
        }
        // Subsequent merge cannot duplicate death loot or split children.
        assert!(state.commit_merge_effects().0.is_empty());
        let mut remaining: Vec<_> = state.entities.slots().collect();
        remaining.reverse();
        for slot in remaining {
            state.remove_entity(slot);
        }
    }
}

#[test]
fn baseline_blueprints_use_exact_registry_dimensions_categories_and_existing_loot() {
    let registry = CoreRegistry::load(None).unwrap();
    for name in BASELINES {
        let id = mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap();
        let facts = registry.entity_types.facts_of(&id).unwrap();
        let blueprint = registry.blueprint(facts.protocol_id).expect(name);
        // Existing custom baselines predate the catalog. All additions use the
        // exact mc-data physical contract rather than generic humanoid boxes.
        if !matches!(
            *name,
            "cow" | "pig" | "sheep" | "chicken" | "zombie" | "spider"
        ) {
            assert_eq!(
                blueprint.half_width,
                facts.dimensions.half_width(),
                "{name}"
            );
            assert_eq!(blueprint.height, facts.dimensions.height, "{name}");
            assert_eq!(Some(blueprint.category), facts.mob_category, "{name}");
        }
        if let Some(table) = &blueprint.loot_table {
            assert!(registry.entity_loot.has_root(table), "{name}");
        }
    }
    let hoglin = registry.blueprint(type_id(&registry, "hoglin")).unwrap();
    assert!(hoglin.has_goal(GoalKind::Breed));
    assert!(hoglin.has_goal(GoalKind::MeleeAttack));
    for name in ["mule", "parrot", "zombie_horse", "camel_husk"] {
        assert_eq!(
            registry
                .blueprint(type_id(&registry, name))
                .unwrap()
                .offspring,
            None
        );
    }
}

#[test]
fn loaded_food_tags_drive_breeding_but_not_horse_healing_food() {
    let mut registry = CoreRegistry::load(None).unwrap();
    let apple = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let horse = type_id(&registry, "horse");
    let horse_food = registry.blueprint(horse).unwrap().breed_food.clone();
    let tags = mc_data::tags::TagsData::from_registries(std::collections::BTreeMap::from([(
        mc_data::Identifier::parse("minecraft:item").unwrap(),
        std::collections::BTreeMap::from([
            (
                mc_data::Identifier::parse("minecraft:cow_food").unwrap(),
                vec![apple as i32],
            ),
            (
                mc_data::Identifier::parse("minecraft:horse_food").unwrap(),
                vec![apple as i32],
            ),
        ]),
    )]));
    crate::species::apply_food_tags(&tags, &mut registry.blueprints);
    for name in ["cow", "mooshroom"] {
        assert_eq!(
            registry
                .blueprint(type_id(&registry, name))
                .unwrap()
                .breed_food,
            vec![apple]
        );
    }
    assert_eq!(registry.blueprint(horse).unwrap().breed_food, horse_food);
    assert!(
        !registry
            .blueprint(horse)
            .unwrap()
            .breed_food
            .contains(&apple)
    );
}

#[test]
fn every_assigned_species_survives_checkpoint_and_core_restart() {
    use crate::config::CoreConfig;
    use crate::core::{Core, SpawnRequest};
    let dir = std::env::temp_dir().join(format!(
        "solaris-species-{}-{}",
        std::process::id(),
        uuid::Uuid::new_v4()
    ));
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    config.simulation_distance = 0;
    let registry = CoreRegistry::load(None).unwrap();
    let expected: std::collections::BTreeSet<_> = BASELINES
        .iter()
        .map(|name| type_id(&registry, name))
        .collect();
    let core = Core::open(config.clone()).unwrap();
    let mut rx = core.subscribe();
    for type_id in &expected {
        while !core.debug_spawn(SpawnRequest {
            type_id: *type_id,
            count: 1,
            spread: false,
            start: 0,
            total: 1,
        }) {
            rx.blocking_recv().expect("core tick");
        }
    }
    loop {
        let output = rx.blocking_recv().expect("spawn tick");
        let found: std::collections::BTreeSet<_> = output
            .snapshot
            .entities
            .iter()
            .map(|entity| entity.type_id)
            .collect();
        if expected.is_subset(&found) {
            break;
        }
    }
    assert!(core.checkpoint());
    core.shutdown();
    let restored = Core::open(config).unwrap();
    let mut rx = restored.subscribe();
    let output = rx.blocking_recv().expect("restored tick");
    let found: std::collections::BTreeSet<_> = output
        .snapshot
        .entities
        .iter()
        .map(|entity| entity.type_id)
        .collect();
    assert!(
        expected.is_subset(&found),
        "missing restored species: {:?}",
        expected.difference(&found).collect::<Vec<_>>()
    );
    restored.shutdown();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn durable_uuid_lookup_tracks_swap_removal_and_rejects_dead_owners() {
    let mut state = state();
    let cow = state.registry.cow_type_id;
    let first = state.spawn_mob(cow, [0.5, 65.0, 0.5], 10.0);
    let second = state.spawn_mob(cow, [1.5, 65.0, 0.5], 10.0);
    let third = state.spawn_mob(cow, [2.5, 65.0, 0.5], 10.0);
    let first_slot = state.entities.slot_of(first).unwrap();
    let first_uuid = state.entities.row(first_slot).uuid;
    let second_slot = state.entities.slot_of(second).unwrap();
    let second_uuid = state.entities.row(second_slot).uuid;
    let third_uuid = state
        .entities
        .row(state.entities.slot_of(third).unwrap())
        .uuid;
    assert_eq!(state.entities.slot_of_uuid(first_uuid), Some(first_slot));
    state.remove_entity(first_slot);
    assert_eq!(state.entities.slot_of_uuid(first_uuid), None);
    assert_eq!(state.entities.slot_of_uuid(third_uuid), Some(first_slot));
    assert_eq!(state.entities.body(first_slot).id, third);
    state.entities.body_mut(second_slot).alive = false;
    assert_eq!(state.entities.slot_of_uuid(second_uuid), None);
    assert_eq!(
        state.entities.slot_of_uuid_any(second_uuid),
        Some(second_slot)
    );
    assert_eq!(state.entities.slot_of_any(second), Some(second_slot));
    state.remove_entity(second_slot);
    assert_eq!(state.entities.slot_of_uuid_any(second_uuid), None);
    assert_eq!(state.entities.slot_of_uuid(third_uuid), Some(first_slot));
}
