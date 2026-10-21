use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::settlements::army::{self, Order};
use mc_data::{Identifier, ItemStack};

fn step(state: &mut TickState, count: usize) {
    for _ in 0..count {
        crate::tick::run_tick(state, &[]);
    }
}
fn player(state: &mut TickState) -> (uuid::Uuid, EntitySlot) {
    let uuid = uuid::Uuid::from_u128(9001);
    crate::tick::run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(44),
            uuid,
            name: "Commander".into(),
            permission_level: 4,
        }],
    );
    let slot = state.entities.slot_of_uuid(uuid).unwrap();
    state.entities.set_pos(slot, [10.5, 201.0, 3.5]);
    (uuid, slot)
}
fn block_state(state: &TickState, name: &str, properties: &[(&str, &str)]) -> u32 {
    let name = Identifier::parse(name).unwrap();
    let default = state.registry.blocks.block(&name).unwrap().default;
    let mut values = state
        .registry
        .blocks
        .by_id(default)
        .unwrap()
        .properties
        .clone();
    for (key, value) in properties {
        values.iter_mut().find(|(name, _)| name == key).unwrap().1 = (*value).into();
    }
    state
        .registry
        .blocks
        .by_name_and_props(&name, &values)
        .unwrap()
        .0
}
#[test]
fn follow_keeps_near_player_and_guard_returns_to_post() {
    let (mut state, id, slot) = army::tests::fixture();
    let soldier = army::recruit(&mut state, id).unwrap();
    let (player, player_slot) = player(&mut state);
    army::order(&mut state, id, Some(soldier), Order::Follow(player)).unwrap();
    step(&mut state, 100);
    assert!(
        distance(state.entities.pos(slot), state.entities.pos(player_slot)) < 9.0,
        "soldier follows commander: {:?}",
        state.entities.pos(slot)
    );
    army::order(&mut state, id, Some(soldier), Order::Guard([4, 201, 4])).unwrap();
    step(&mut state, 100);
    assert!(
        distance(state.entities.pos(slot), [4.5, 201.0, 4.5]) < 3.0,
        "soldier returns to guard post: {:?}",
        state.entities.pos(slot)
    );
}
#[test]
fn patrol_goals_and_motion_stay_inside_settlement_radius() {
    let (mut state, id, slot) = army::tests::fixture();
    state.settlements.get_mut(id).unwrap().radius = 6;
    state
        .settlements
        .get_mut(id)
        .unwrap()
        .buildings
        .values_mut()
        .find(|b| b.kind == crate::settlements::buildings::BuildingKind::Barracks)
        .unwrap()
        .bounds = mc_script::ScriptWorldBounds::new([4, 200, 4], [6, 204, 6]).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 8.5]);
    let initial = state.entities.pos(slot);
    let mut moved = false;
    army::recruit(&mut state, id).unwrap();
    player(&mut state);
    let anchor = state.settlements.get(id).unwrap().anchor.map(f64::from);
    let radius = f64::from(state.settlements.get(id).unwrap().radius);
    for _ in 0..200 {
        let v = state.entities.row(slot).villager.as_ref().unwrap();
        assert!(distance(goal(&state, slot, v).unwrap(), anchor) <= radius * radius);
        step(&mut state, 1);
        assert!(distance(state.entities.pos(slot), anchor) <= radius * radius);
        moved |= distance(state.entities.pos(slot), initial) > 1.0;
    }
    assert!(
        moved,
        "patrol must physically move, not merely choose an in-radius goal"
    );
}
#[test]
fn soldier_kills_zombie_attacking_a_villager_through_native_damage() {
    let (mut state, id, slot) = army::tests::fixture();
    let sword = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:diamond_sword").unwrap())
        .unwrap();
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    let soldier = army::recruit(&mut state, id).unwrap();
    player(&mut state);
    army::order(&mut state, id, Some(soldier), Order::Guard([4, 201, 4])).unwrap();
    let villager = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let victim = state.spawn_mob(villager, [4.5, 201.0, 4.5], 20.0);
    let victim_slot = state.entities.slot_of(victim).unwrap();
    state
        .entities
        .row_mut(victim_slot)
        .villager
        .as_mut()
        .unwrap()
        .settlement = Some(id);
    let zombie = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let enemy = state.spawn_mob(zombie, [5.0, 201.0, 4.5], 20.0);
    let mut saw_damage = false;
    let defender = state.entities.body(slot).id;
    let mut resident_was_attacked = false;
    for _ in 0..160 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        saw_damage |= output
            .changes
            .entity_events
            .iter()
            .any(|e| matches!(e,EntityEvent::Damage{entity,cause:Some(cause),..} if *entity==enemy && *cause==defender));
        resident_was_attacked |= output.changes.entity_events.iter()
            .any(|e| matches!(e,EntityEvent::Damage{entity,cause:Some(cause),..} if *entity==victim && *cause==enemy));
        if state.entities.slot_of(enemy).is_none() {
            break;
        }
    }
    assert!(saw_damage, "native hurt event must be published");
    assert!(
        resident_was_attacked,
        "zombie must actually attack the defended villager"
    );
    assert!(
        state
            .entities
            .slot_of(enemy)
            .is_none_or(|s| !state.entities.body(s).alive),
        "soldier must kill zombie"
    );
    assert!(state.entities.body(slot).alive);
    assert!(
        state
            .entities
            .slot_of(victim)
            .is_some_and(|s| state.entities.body(s).alive),
        "defended resident survives"
    );
}

#[test]
fn guard_soldier_with_iron_sword_kills_nearby_hostile_then_returns_to_post() {
    let (mut state, settlement, slot) = army::tests::fixture();
    let sword = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:iron_sword").unwrap())
        .unwrap();
    state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots[0] = ItemStack::new(sword, 1);
    let soldier = army::recruit(&mut state, settlement).unwrap();
    player(&mut state);
    let post = [4.5, 201.0, 4.5];
    army::order(
        &mut state,
        settlement,
        Some(soldier),
        Order::Guard([4, 201, 4]),
    )
    .unwrap();
    let zombie = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let enemy = state.spawn_mob(zombie, [9.5, 201.0, 4.5], 20.0);
    let enemy_slot = state.entities.slot_of(enemy).unwrap();
    state.entities.row_mut(enemy_slot).controls.no_ai = true;
    let defender = state.entities.body(slot).id;
    let mut swung = false;
    for _ in 0..200 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        swung |=
            output.changes.entity_events.iter().any(
                |event| matches!(event, EntityEvent::Swing { entity, .. } if *entity == defender),
            );
        if state
            .entities
            .slot_of(enemy)
            .is_none_or(|enemy_slot| !state.entities.body(enemy_slot).alive)
        {
            break;
        }
    }
    assert!(
        swung,
        "guard soldier must attack a hostile five blocks from its post"
    );
    assert!(
        state
            .entities
            .slot_of(enemy)
            .is_none_or(|enemy_slot| !state.entities.body(enemy_slot).alive),
        "guard soldier with an iron sword must kill the hostile within 200 ticks"
    );
    let returned = (0..200).any(|_| {
        crate::tick::run_tick(&mut state, &[]);
        distance(state.entities.pos(slot), post) < 3.0
    });
    assert!(
        returned,
        "guard soldier must return to its post within 200 ticks"
    );
}

#[test]
fn guard_squad_kills_nearby_zombie_within_200_ticks() {
    let (mut state, settlement, _) = army::tests::fixture();
    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .values_mut()
        .find(|building| building.kind == crate::settlements::buildings::BuildingKind::Barracks)
        .unwrap()
        .level = 2;
    let villager = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    for position in [[2.5, 201.0, 3.5], [3.5, 201.0, 2.5], [4.5, 201.0, 3.5]] {
        let entity = state.spawn_mob(villager, position, 20.0);
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.row_mut(slot).villager = Some(Box::new(VillagerState {
            settlement: Some(settlement),
            ..Default::default()
        }));
    }
    let sword = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:iron_sword").unwrap())
        .unwrap();
    let storage = &mut state.block_entities.get_mut(&(6, 201, 6)).unwrap().slots;
    for stack in storage.iter_mut().take(4) {
        *stack = ItemStack::new(sword, 1);
    }
    for _ in 0..4 {
        army::recruit(&mut state, settlement).unwrap();
    }
    player(&mut state);
    state.day_time = 18_000;
    army::order(&mut state, settlement, None, Order::Guard([4, 201, 4])).unwrap();
    let zombie = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let enemy = state.spawn_mob(zombie, [7.5, 201.0, 4.5], 20.0);
    let enemy_slot = state.entities.slot_of(enemy).unwrap();
    state.entities.row_mut(enemy_slot).controls.no_ai = true;
    for _ in 0..200 {
        crate::tick::run_tick(&mut state, &[]);
        if state
            .entities
            .slot_of(enemy)
            .is_none_or(|enemy_slot| !state.entities.body(enemy_slot).alive)
        {
            break;
        }
    }
    assert!(
        state
            .entities
            .slot_of(enemy)
            .is_none_or(|enemy_slot| !state.entities.body(enemy_slot).alive),
        "four iron-sword guards must kill a nearby zombie within 200 ticks"
    );
}

#[test]
fn full_tick_soldier_attack_never_leaks_slot_damage_to_next_tick() {
    let (mut state, settlement, slot) = army::tests::fixture();
    let soldier = army::recruit(&mut state, settlement).unwrap();
    player(&mut state);
    state.entities.set_pos(slot, [3.5, 201.0, 3.5]);
    army::order(
        &mut state,
        settlement,
        Some(soldier),
        Order::Guard([3, 201, 3]),
    )
    .unwrap();
    let zombie = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let enemy = state.spawn_mob(zombie, [4.0, 201.0, 3.5], 20.0);
    let enemy_slot = state.entities.slot_of(enemy).unwrap();
    state.entities.row_mut(enemy_slot).controls.no_ai = true;
    let defender = state.entities.body(slot).id;
    let (attacking_tick, _, _) = crate::tick::run_tick(&mut state, &[]);
    assert!(
        attacking_tick
            .changes
            .entity_events
            .iter()
            .any(|event| matches!(event, EntityEvent::Swing { entity, .. } if *entity == defender)),
        "fixture must execute a soldier attack"
    );

    // Between full ticks, removal permutes the dense slot exactly as natural
    // despawn/input removal does before the next damage commit.
    let cow = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let innocent = state.spawn_mob(cow, [12.5, 201.0, 12.5], 20.0);
    let innocent_slot = state.entities.slot_of(innocent).unwrap();
    state.entities.row_mut(innocent_slot).controls.no_ai = true;
    let removed_slot = state.entities.slot_of(enemy).unwrap();
    state.remove_entity(removed_slot);
    assert_eq!(state.entities.slot_of(innocent), Some(removed_slot));
    crate::tick::run_tick(&mut state, &[]);
    let innocent_slot = state.entities.slot_of(innocent).unwrap();
    assert_eq!(
        state.entities.row(innocent_slot).health,
        20.0,
        "a soldier hit must never follow a removed target's old dense slot"
    );
    assert!(
        state.merge_damage.is_empty(),
        "full tick must drain all admitted hits"
    );
    assert!(attacking_tick.changes.entity_events.iter().any(|event|
        matches!(event, EntityEvent::Damage { entity, cause: Some(cause), .. } if *entity == enemy && *cause == defender)),
        "the attack must commit against its intended entity in its own tick");
}

#[test]
fn full_tick_army_orders_cross_loaded_chunks() {
    let (mut state, settlement, soldier_slot) = army::tests::fixture();
    for x in 0..=2 {
        state.tickets.insert(
            format!("army-order-chunk-{x}"),
            (crate::boundary::ChunkPos { x, z: 0 }, 0),
        );
    }
    state
        .world
        .ensure_blocking(&rustc_hash::FxHashMap::from_iter(
            (0..=2).map(|x| (crate::boundary::ChunkPos { x, z: 0 }, 0)),
        ));
    for x in 0..48 {
        for z in 0..16 {
            state
                .world
                .set_block(x, 200, z, state.registry.states.stone);
            for y in 201..206 {
                state.world.set_block(x, y, z, state.registry.states.air);
            }
        }
    }
    let door = [5, 201, 3];
    for x in 1..=5 {
        for z in 1..=5 {
            if x != 1 && x != 5 && z != 1 && z != 5 {
                continue;
            }
            for y in 201..=203 {
                if [x, y, z] != door && [x, y, z] != [door[0], door[1] + 1, door[2]] {
                    state.world.set_block(x, y, z, state.registry.states.stone);
                }
            }
        }
    }
    for (y, half) in [(201, "lower"), (202, "upper")] {
        state.world.set_block(
            door[0],
            y,
            door[2],
            block_state(
                &state,
                "minecraft:oak_door",
                &[("facing", "west"), ("half", half), ("open", "false")],
            ),
        );
    }
    state.world.fold_overlay();
    state.refresh_active_chunks(state.tick);

    state
        .settlements
        .get_mut(settlement)
        .unwrap()
        .buildings
        .values_mut()
        .find(|building| building.kind == crate::settlements::buildings::BuildingKind::Barracks)
        .unwrap()
        .level = 2;
    let villager = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let mut soldier_slots = vec![soldier_slot];
    for _ in 0..3 {
        let entity = state.spawn_mob(villager, [3.5, 201.0, 3.5], 20.0);
        let slot = state.entities.slot_of(entity).unwrap();
        state.entities.row_mut(slot).villager = Some(Box::new(VillagerState {
            settlement: Some(settlement),
            ..Default::default()
        }));
        soldier_slots.push(slot);
    }
    let soldiers: Vec<_> = (0..4)
        .map(|_| army::recruit(&mut state, settlement).unwrap())
        .collect();
    let (commander, commander_slot) = player(&mut state);
    state.entities.set_pos(commander_slot, [38.5, 201.0, 3.5]);
    assert_eq!(
        army::order(&mut state, settlement, None, Order::Follow(commander)).unwrap(),
        soldiers.len()
    );
    let follow_start: Vec<_> = soldier_slots
        .iter()
        .map(|&slot| distance(state.entities.pos(slot), state.entities.pos(commander_slot)))
        .collect();
    step(&mut state, 600);
    for (&slot, start) in soldier_slots.iter().zip(follow_start) {
        let current = distance(state.entities.pos(slot), state.entities.pos(commander_slot));
        assert!(
            current < 9.0,
            "follow must close a 35-block player gap: {:?}",
            state.entities.pos(slot)
        );
        assert!(current < start, "follow must make real progress");
    }

    let guard_post = [4.5, 201.0, 12.5];
    let guard_start: Vec<_> = soldier_slots
        .iter()
        .map(|&slot| distance(state.entities.pos(slot), guard_post))
        .collect();
    assert_eq!(
        army::order(&mut state, settlement, None, Order::Guard([4, 201, 12])).unwrap(),
        soldiers.len()
    );
    step(&mut state, 600);
    for (&slot, start) in soldier_slots.iter().zip(guard_start) {
        assert!(
            distance(state.entities.pos(slot), guard_post) < start,
            "guard must close distance to its post: {:?}",
            state.entities.pos(slot)
        );
    }

    assert_eq!(
        army::order(&mut state, settlement, None, Order::Patrol).unwrap(),
        soldiers.len()
    );
    let patrol_start: Vec<_> = soldier_slots
        .iter()
        .map(|&slot| state.entities.pos(slot))
        .collect();
    step(&mut state, 240);
    let anchor = state
        .settlements
        .get(settlement)
        .unwrap()
        .anchor
        .map(f64::from);
    for (&slot, start) in soldier_slots.iter().zip(patrol_start) {
        assert!(
            distance(state.entities.pos(slot), anchor) <= 16.0_f64.powi(2),
            "patrol must remain inside the settlement"
        );
        assert!(
            distance(state.entities.pos(slot), start) > 1.0,
            "patrol must start navigation, not remain frozen"
        );
    }
}
