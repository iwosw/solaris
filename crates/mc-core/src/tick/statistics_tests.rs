use super::*;
use crate::boundary::{EntityId, Hand, PlayerChange};
use mc_data::statistics::{CustomStat, StatType};

#[test]
fn statistics_movement_uses_accepted_centimeters_and_modes() {
    let (mut state, entity, pos) = flat_player_fixture();
    let move_to = |position| PlayerInput::Move {
        connection: ConnectionId(1),
        position,
        yaw: 0.0,
        pitch: 0.0,
        on_ground: true,
    };
    run_tick(&mut state, &[move_to([pos[0] + 1.234, pos[1], pos[2]])]);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::WalkOneCm),
        123
    );
    run_tick(&mut state, &[move_to([pos[0] + 100.0, pos[1], pos[2]])]);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::WalkOneCm),
        123
    );
    run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection: ConnectionId(1),
                teleport_id: 2,
            },
            PlayerInput::Sprint {
                connection: ConnectionId(1),
                sprinting: true,
            },
            move_to([pos[0] + 2.234, pos[1], pos[2]]),
        ],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::SprintOneCm),
        100
    );
    run_tick(
        &mut state,
        &[
            PlayerInput::Sprint {
                connection: ConnectionId(1),
                sprinting: false,
            },
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: true,
            },
            move_to([pos[0] + 3.234, pos[1], pos[2]]),
        ],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::CrouchOneCm),
        100
    );
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: [pos[0] + 3.234, pos[1] + 0.42, pos[2]],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::Jump),
        1
    );
}

#[test]
fn statistics_break_pickup_drop_and_request_are_authoritative() {
    let (mut state, entity, pos) = flat_player_fixture();
    let target = crate::boundary::BlockPos {
        x: pos[0].floor() as i32 + 1,
        y: pos[1] as i32,
        z: pos[2].floor() as i32,
    };
    let stone = state.registry.states.stone;
    let mut requires_tool = vec![false; stone as usize + 1];
    requires_tool[stone as usize] = true;
    std::sync::Arc::get_mut(&mut state.registry)
        .expect("fixture owns registry")
        .mining = mc_data::block_mining::BlockMiningTable::from_arrays(
        "26.1.2",
        vec![1.5; requires_tool.len()],
        requires_tool,
    );
    let raw_stone = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(stone))
        .unwrap()
        .block
        .raw_id as u32;
    state.world.set_block(target.x, target.y, target.z, stone);
    let pickaxe = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_pickaxe").unwrap())
        .unwrap();
    state.players.get_mut(&entity).unwrap().inventory.slots[0] =
        mc_data::ItemStack::new(pickaxe, 1);
    run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: target,
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Mined, raw_stone),
        1
    );
    run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: target,
            action: DigAction::Finish,
            sequence: 2,
        }],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Mined, raw_stone),
        1
    );
    // A real accepted break with the wrong tool is not BLOCK_MINED in vanilla.
    state.world.set_block(target.x, target.y, target.z, stone);
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = mc_data::ItemStack::EMPTY;
    run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: target,
            action: DigAction::Finish,
            sequence: 3,
        }],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Mined, raw_stone),
        1
    );
    let apple = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:apple").unwrap())
        .unwrap();
    let dropped =
        crate::tick::spawn_item_entity(&mut state, pos, mc_data::ItemStack::new(apple, 3));
    let item_slot = state.entities.slot_of(dropped).unwrap();
    let collector = state.entities.slot_of(entity).unwrap();
    state.merge_pickups.push((item_slot, collector, 3));
    state.commit_merge_effects();
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::PickedUp, apple),
        3
    );
    state.players.get_mut(&entity).unwrap().inventory.slots[0] = mc_data::ItemStack::new(apple, 3);
    run_tick(
        &mut state,
        &[PlayerInput::DropSelected {
            connection: ConnectionId(1),
            whole_stack: true,
        }],
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get(StatType::Dropped, apple),
        3
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::Drop),
        1
    );
    let expected = state.players[&entity].statistics.wire_entries();
    let (_, outbox, _) = run_tick(
        &mut state,
        &[PlayerInput::RequestStats {
            connection: ConnectionId(1),
        }],
    );
    let awarded = outbox
        .iter()
        .flat_map(|(_, changes)| changes)
        .find_map(|change| match change {
            PlayerChange::Statistics { stats, .. } => Some(stats),
            _ => None,
        })
        .expect("statistics response");
    assert_eq!(awarded, &expected);
    assert!(awarded.iter().all(|(_, _, value)| *value != 0));
}

#[test]
fn statistics_damage_absorption_resistance_death_and_breakage() {
    let (mut state, entity, _) = flat_player_fixture();
    state.tick = 100;
    let slot = state.entities.slot_of(entity).unwrap();
    let player = state.players.get_mut(&entity).unwrap();
    player
        .active_effects
        .entries
        .push(crate::effects::ActiveEffect::new(
            crate::effects::EffectId::Resistance,
            200,
            0,
        ));
    player.active_effects.absorption = 2.0;
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 10.0,
            kind: crate::survival::DamageKind::Generic,
            source: None,
        },
    ));
    state.commit_merge_effects();
    let stats = &state.players[&entity].statistics;
    assert_eq!(stats.get_custom(CustomStat::DamageTaken), 60);
    assert_eq!(stats.get_custom(CustomStat::DamageAbsorbed), 20);
    assert_eq!(stats.get_custom(CustomStat::DamageResisted), 20);
    let pick = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:wooden_pickaxe").unwrap())
        .unwrap();
    let rules = state.registry.item_rules();
    let player = state.players.get_mut(&entity).unwrap();
    player.inventory.slots[0] = mc_data::ItemStack::new(pick, 1).with_damage(58);
    player.damage_held_tool(&rules, 1);
    assert_eq!(player.statistics.get(StatType::Broken, pick), 1);
    player.fall(-3.5, false, false);
    player.fall(0.0, true, false);
    assert_eq!(player.statistics.get_custom(CustomStat::FallOneCm), 350);
    player.statistics.set_custom(CustomStat::TimeSinceDeath, 50);
    player.die(&rules, true);
    assert_eq!(player.statistics.get_custom(CustomStat::Deaths), 1);
    assert_eq!(player.statistics.get_custom(CustomStat::TimeSinceDeath), 0);
    player.inventory.slots[0] = mc_data::ItemStack::new(pick, 1);
    player.consume_held(Hand::Main, 1);
    assert_eq!(player.statistics.get(StatType::Used, pick), 1);
}

#[test]
fn statistics_swimming_climbing_flying_and_riding_use_vanilla_distance_dimensions() {
    let (mut state, entity, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_ground(slot, false);
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        state.registry.states.water,
    );
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).pose = 3;
    let diagonal = [pos[0] + 0.3, pos[1] + 0.4, pos[2]];
    crate::statistics::movement(&mut state, entity, slot, pos, diagonal, false);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::SwimOneCm),
        50
    );
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
        state.registry.states.air,
    );
    std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance).pose = 0;
    crate::statistics::movement(
        &mut state,
        entity,
        slot,
        pos,
        [pos[0] + 0.25, pos[1] + 1.0, pos[2]],
        false,
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::FlyOneCm),
        0
    );
    crate::statistics::movement(
        &mut state,
        entity,
        slot,
        pos,
        [pos[0] + 0.26, pos[1] + 1.0, pos[2]],
        false,
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::FlyOneCm),
        26
    );
    let horse = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:horse").unwrap())
        .unwrap();
    let mount = state.spawn_mob(horse, pos, 20.0);
    let mount_slot = state.entities.slot_of(mount).unwrap();
    crate::statistics::riding(&mut state, entity, mount_slot, pos, diagonal);
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::HorseOneCm),
        50
    );
    for name in [
        "minecraft:camel",
        "minecraft:llama",
        "minecraft:trader_llama",
    ] {
        let kind = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        let mount = state.spawn_mob(kind, pos, 20.0);
        let mount_slot = state.entities.slot_of(mount).unwrap();
        crate::statistics::riding(&mut state, entity, mount_slot, pos, diagonal);
    }
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::HorseOneCm),
        200
    );
    let ladder = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:ladder").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(
        pos[0].floor() as i32,
        pos[1].floor() as i32 + 1,
        pos[2].floor() as i32,
        ladder,
    );
    crate::statistics::movement(
        &mut state,
        entity,
        slot,
        pos,
        [pos[0], pos[1] + 1.0, pos[2]],
        false,
    );
    assert_eq!(
        state.players[&entity]
            .statistics
            .get_custom(CustomStat::ClimbOneCm),
        100
    );
}

#[test]
fn statistics_kills_and_damage_credit_actual_attacker_and_entity_type() {
    let (mut state, player, pos) = flat_player_fixture();
    state.tick = 100;
    let zombie = state.registry.zombie_type_id;
    let mob = state.spawn_mob(zombie, [pos[0] + 1.0, pos[1], pos[2]], 20.0);
    let mob_slot = state.entities.slot_of(mob).unwrap();
    let melee = |attacker, position| {
        Some(crate::intent::DamageSource::Melee {
            attacker,
            position,
            yaw: 0.0,
            bonus_knockback: 0.0,
            sweep: false,
            fire_ticks: 0,
            bane_ticks: 0,
        })
    };
    state.merge_damage.push((
        mob_slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Melee,
            source: melee(player, pos),
        },
    ));
    state.commit_merge_effects();
    let stats = &state.players[&player].statistics;
    assert_eq!(stats.get(StatType::Killed, zombie), 1);
    assert_eq!(stats.get_custom(CustomStat::MobKills), 1);
    assert_eq!(stats.get_custom(CustomStat::DamageDealt), 200);
    let killer = state.spawn_mob(zombie, [pos[0] + 1.0, pos[1], pos[2]], 20.0);
    let player_slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        player_slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Melee,
            source: melee(killer, [pos[0] + 1.0, pos[1], pos[2]]),
        },
    ));
    state.commit_merge_effects();
    let stats = &state.players[&player].statistics;
    assert_eq!(stats.get(StatType::KilledBy, zombie), 1);
    assert_eq!(stats.get_custom(CustomStat::Deaths), 1);
}

#[test]
fn statistics_ownerless_projectile_does_not_become_living_kill_credit() {
    let (mut state, player, pos) = flat_player_fixture();
    state.tick = 100; // The join-protection window must not refuse the hit.
    let arrow = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let projectile = state.spawn_mob(arrow, pos, 0.0);
    let player_slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        player_slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Projectile,
            source: Some(crate::intent::DamageSource::Projectile {
                projectile,
                direct_type_id: arrow,
                owner: projectile,
                velocity: [1.0, 0.0],
                punch: 0,
                flame: false,
            }),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::Deaths),
        1
    );
    assert_eq!(
        state.players[&player]
            .statistics
            .get(StatType::KilledBy, arrow),
        0
    );
}

#[test]
fn statistics_sweep_only_credits_absorbed_and_resisted_damage() {
    let (mut state, player, pos) = flat_player_fixture();
    let source = crate::intent::DamageSource::Sweep {
        attacker: player,
        primary: EntityId(500),
        position: pos,
        yaw: 0.0,
        fire_ticks: 0,
        bane_ticks: 0,
    };
    crate::statistics::damage_dealt(&mut state.players, Some(source), 2.0, 1.0, 1.0);
    let statistics = &state.players[&player].statistics;
    assert_eq!(statistics.get_custom(CustomStat::DamageDealt), 0);
    assert_eq!(statistics.get_custom(CustomStat::DamageDealtAbsorbed), 10);
    assert_eq!(statistics.get_custom(CustomStat::DamageDealtResisted), 10);
}

#[test]
fn statistics_recent_player_hit_credits_environmental_finishing_kill() {
    let (mut state, player, pos) = flat_player_fixture();
    let zombie = state.registry.zombie_type_id;
    let mob = state.spawn_mob(zombie, [pos[0] + 1.0, pos[1], pos[2]], 20.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 1.0,
            kind: crate::survival::DamageKind::Melee,
            source: Some(crate::intent::DamageSource::Melee {
                attacker: player,
                position: pos,
                yaw: 0.0,
                bonus_knockback: 0.0,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
    state.commit_merge_effects();
    state.tick += 1;
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
            kind: crate::survival::DamageKind::Generic,
            source: None,
        },
    ));
    state.commit_merge_effects();
    assert_eq!(
        state.players[&player]
            .statistics
            .get(StatType::Killed, zombie),
        1
    );
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::MobKills),
        1
    );
    assert_eq!(
        state.players[&player]
            .statistics
            .get_custom(CustomStat::DamageDealt),
        9 // Accepted post-armor health loss, rounded to tenths.
    );
}
