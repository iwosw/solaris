use super::conversion;
use crate::{
    boundary::{ConnectionId, Hand, PlayerInput},
    effects::{ActiveEffect, EffectId},
    registry::CoreRegistry,
    tick::{TickState, run_tick},
    world::ChunkWorld,
};
use std::sync::Arc;
pub(super) fn state() -> TickState {
    let registry = Arc::new(CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [0, 80, 0],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 1, 512)
}
fn entity_type(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}
#[test]
fn zombie_infection_preserves_identity_trade_data_and_prevents_death_drops() {
    let mut state = state();
    state.difficulty = crate::survival::Difficulty::Hard;
    let villager_type = entity_type(&state, "minecraft:villager");
    let zombie_type = entity_type(&state, "minecraft:zombie");
    let villager = state.spawn_mob(villager_type, [0.0, 80.0, 0.0], 20.0);
    let zombie = state.spawn_mob(zombie_type, [1.0, 80.0, 0.0], 20.0);
    let slot = state.entities.slot_of(villager).unwrap();
    state.entities.row_mut(slot).villager.as_mut().unwrap().xp = 73;
    let uuid = state.entities.row(slot).uuid;
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Melee {
                attacker: zombie,
                position: [1.0, 80.0, 0.0],
                yaw: 0.0,
                bonus_knockback: 0.0,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
    let (deaths, events, spawns, _) = state.commit_merge_effects();
    assert!(deaths.is_empty());
    assert!(spawns.is_empty());
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, crate::boundary::EntityEvent::Death { .. }))
    );
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert_eq!(row.uuid, uuid);
    assert!(body.alive);
    assert_eq!(row.health, 20.0);
    assert_eq!(row.villager.as_ref().unwrap().xp, 73);
    assert_eq!(row.villager.as_ref().unwrap().conversion_time, Some(-1));
}
#[test]
fn cure_requires_weakness_consumes_one_apple_once_and_restores_villager() {
    let mut state = state();
    let connection = ConnectionId(1);
    let uuid = uuid::Uuid::from_u128(91);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection,
            uuid,
            name: "cure".to_owned(),
            permission_level: 0,
        }],
    );
    let player = state.connections.entity_of(connection).unwrap();
    let zombie_type = entity_type(&state, "minecraft:zombie_villager");
    let zombie = state.spawn_mob(zombie_type, [0.0, 80.0, 0.0], 20.0);
    let slot = state.entities.slot_of(zombie).unwrap();
    let apple = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:golden_apple").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(apple, 2);
    assert!(!conversion::interact(&mut state, player, slot, Hand::Main));
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    state
        .entities
        .row_mut(slot)
        .active_effects
        .add(ActiveEffect::new(EffectId::Weakness, 100, 0));
    assert!(conversion::interact(&mut state, player, slot, Hand::Main));
    assert!(!conversion::interact(&mut state, player, slot, Hand::Main));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert!(
        !state
            .entities
            .row(slot)
            .active_effects
            .has(EffectId::Weakness)
    );
    state
        .entities
        .row_mut(slot)
        .villager
        .as_mut()
        .unwrap()
        .conversion_time = Some(1);
    conversion::tick(&mut state);
    let body = state.entities.body(slot);
    let row = state.entities.row(slot);
    assert_eq!(
        body.kind,
        crate::entities::EntityKind::Mob {
            type_id: entity_type(&state, "minecraft:villager")
        }
    );
    assert_eq!(row.villager.as_ref().unwrap().conversion_time, None);
    assert!(row.active_effects.has(EffectId::Nausea));
    assert!(
        row.villager
            .as_ref()
            .unwrap()
            .gossips
            .iter()
            .any(|g| g.target == uuid && g.kind == "major_positive" && g.value == 20)
    );
}
#[test]
fn easy_zombie_kill_does_not_infect() {
    let mut state = state();
    let villager_type = entity_type(&state, "minecraft:villager");
    let zombie_type = entity_type(&state, "minecraft:zombie");
    let villager = state.spawn_mob(villager_type, [0.0, 80.0, 0.0], 20.0);
    let zombie = state.spawn_mob(zombie_type, [1.0, 80.0, 0.0], 20.0);
    let slot = state.entities.slot_of(villager).unwrap();
    assert!(!conversion::infect(&mut state, slot, Some(zombie)));
}
