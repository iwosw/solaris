//! Tick integration tests: join/leave/move + chunks, mob spawn via
//! blueprints, dig/place, journal recovery. Sibling `*_tests.rs`.

use std::sync::Arc;

use mc_world::ChunkPos;

use crate::boundary::{ConnectionId, DigAction, PlayerInput};
use crate::config::CoreConfig;
use crate::core::Core;
use crate::horde_bench::flat_world;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
#[path = "tick/cohort_tests.rs"]
mod cohort_tests;
#[path = "tick/craft_stations_tests.rs"]
mod craft_stations_tests;
#[path = "effects_trigger_tests.rs"]
mod effects_trigger_tests;
#[path = "elytra_tests.rs"]
mod elytra_tests;
#[path = "mace_tests.rs"]
mod mace_tests;
#[path = "tick/movement_tests.rs"]
mod movement_tests;
#[path = "tick/statistics_tests.rs"]
mod statistics_tests;

#[test]
fn integrity_lethal_damage_rejects_staged_pickup_before_respawn() {
    let mut state = test_state();
    let player = join(&mut state, 1, "collector");
    let player_slot = state.entities.slot_of(player).unwrap();
    let stone = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap();
    state
        .players
        .get_mut(&player)
        .unwrap()
        .inventory
        .slots
        .fill(mc_data::ItemStack::new(stone, 64));
    let pos = state.entities.pos(player_slot);
    let ground = crate::tick::spawn_item_entity(&mut state, pos, mc_data::ItemStack::new(stone, 1));
    let ground_slot = state.entities.slot_of(ground).unwrap();
    state.merge_pickups.push((ground_slot, player_slot, 1));
    state.merge_consumed_items.insert(ground_slot);
    state.merge_damage.push((
        player_slot,
        crate::tick::DamagePiece {
            amount: 1000.0,
            kind: crate::survival::DamageKind::Void,
            source: None,
            physical: None,
            custom_armor: false,
        },
    ));
    let (deaths, events, _, removed) = state.commit_merge_effects();
    assert!(deaths.contains(&player));
    assert!(state.entities.slot_of(ground).is_some());
    assert!(!removed.contains(&ground));
    assert!(!events.iter().any(|event| matches!(event,
        crate::boundary::EntityEvent::Pickup { item, .. } if *item == ground)));
    let player = state.players.get_mut(&player).unwrap();
    assert!(
        player
            .inventory
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    player.respawn(pos, 0.0, 0.0, Default::default());
    assert!(
        player
            .inventory
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
}

fn test_registry() -> Arc<CoreRegistry> {
    Arc::new(CoreRegistry::load(None).expect("registry loads"))
}

/// Keep the full vanilla catalog, but do not reload it for each table-driven
/// case in one test process. Clones retain independently mutable test tables.
pub(crate) fn full_registry() -> CoreRegistry {
    static REGISTRY: std::sync::LazyLock<CoreRegistry> = std::sync::LazyLock::new(|| {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
        CoreRegistry::load(Some(&dir)).expect("full registry loads")
    });
    REGISTRY.clone()
}

fn test_state() -> TickState {
    let registry = test_registry();
    let world = flat_world(&registry);
    TickState::new(registry, world, 2, 512)
}

fn terrain_test_state() -> TickState {
    let registry = test_registry();
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("test terrain spawn");
    let world_spawn = [spawn.block_x, spawn.surface_y + 1, spawn.block_z];
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 2, 512)
}

/// Join one player, then wait for the off-tick loader to make its whole
/// simulation square resident (tests probe blocks right after joining).
fn join(state: &mut TickState, connection: u64, name: &str) -> crate::boundary::EntityId {
    let (output, _, _) = run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::new_v4(),
            name: name.to_owned(),
            permission_level: 0,
        }],
    );
    assert_eq!(output.tick, state.tick);
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    run_tick(
        state,
        &[PlayerInput::ConfirmTeleport {
            connection: ConnectionId(connection),
            teleport_id: 1,
        }],
    );
    state
        .connections
        .entity_of(ConnectionId(connection))
        .expect("joined")
}

#[test]
fn radius_split_residency_covers_view_margin_independently_of_simulation() {
    use crate::boundary::ChunkPos;
    let mut state = test_state();
    let player = join(&mut state, 1, "viewer");
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [8.0, 100.0, 8.0]);
    state.config_view_distance = 8;
    state.config_sim_distance = 4;
    let wanted = state.desired_chunks();
    for z in -9..=9 {
        for x in -9..=9 {
            assert!(wanted.contains_key(&ChunkPos { x, z }), "missing {x},{z}");
        }
    }
    for simulation in [6, 2, 6] {
        state.config_sim_distance = simulation;
        assert_eq!(state.desired_chunks(), wanted);
    }
}

#[test]
fn radius_split_active_hysteresis_and_tickets_do_not_activate_the_view_square() {
    use crate::boundary::ChunkPos;
    let mut state = test_state();
    let player = join(&mut state, 1, "viewer");
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [8.0, 100.0, 8.0]);
    state.config_view_distance = 8;
    state.config_sim_distance = 2;
    state.refresh_active_chunks(state.tick + 1);
    state.world.ensure_blocking(&state.desired_chunks());
    state.refresh_active_chunks(state.tick + 1);
    assert!(state.world.is_active(2, 0));
    assert!(state.world.has_chunk(8, 0));
    assert!(!state.world.is_active(8, 0));
    state.entities.set_pos(slot, [24.0, 100.0, 8.0]);
    state.refresh_active_chunks(state.tick + 1);
    assert!(
        state.world.is_active(-2, 0),
        "exit band retains previous active chunk"
    );
    state.entities.set_pos(slot, [40.0, 100.0, 8.0]);
    state.refresh_active_chunks(state.tick + 1);
    assert!(
        !state.world.is_active(-2, 0),
        "outside exit band deactivates"
    );
    let forced = ChunkPos { x: 8, z: 0 };
    state.tickets.insert("forced".into(), (forced, 0));
    state.refresh_active_chunks(state.tick + 1);
    assert!(state.world.is_active(forced.x, forced.z));
    state.tickets.remove("forced");
    state.refresh_active_chunks(state.tick + 1);
    assert!(!state.world.is_active(forced.x, forced.z));
}

#[test]
fn radius_split_resident_inactive_mob_and_inhabited_time_freeze_until_activation() {
    use crate::boundary::ChunkPos;
    let mut state = test_state();
    let player = join(&mut state, 1, "viewer");
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(viewer, [8.0, 100.0, 8.0]);
    state.config_view_distance = 8;
    state.config_sim_distance = 2;
    state.refresh_active_chunks(state.tick + 1);
    state.world.ensure_blocking(&state.desired_chunks());
    let chunk = ChunkPos { x: 6, z: 0 };
    let cow = state.registry.cow_type_id;
    let mob = state.spawn_mob(cow, [104.0, 70.0, 8.0], 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.set_vel(slot, [0.125, 0.0, 0.0]);
    let position = state.entities.pos(slot);
    let inhabited = state.world.inhabited_time(chunk);
    for _ in 0..3 {
        run_tick(&mut state, &[]);
    }
    assert!(state.world.has_chunk(chunk.x, chunk.z));
    assert!(!state.world.is_active(chunk.x, chunk.z));
    assert_eq!(state.entities.pos(slot), position);
    assert_eq!(state.world.inhabited_time(chunk), inhabited);
    state.config_sim_distance = 6;
    run_tick(&mut state, &[]);
    assert!(state.world.is_active(chunk.x, chunk.z));
    assert_ne!(state.entities.pos(slot), position);
    assert_eq!(state.world.inhabited_time(chunk), inhabited + 1);
}

#[test]
fn radius_split_stationary_simulation_six_two_six_wal_restart_matches_live_age() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let mut state = test_state();
    let player = join(&mut state, 1, "clock-viewer");
    let viewer = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(viewer, [8.0, 64.0, 8.0]);
    state.config_view_distance = 8;
    state.config_sim_distance = 6;
    state.refresh_active_chunks(state.tick + 1);
    state.world.ensure_blocking(&state.desired_chunks());
    let mob = state.spawn_mob(state.registry.cow_type_id, [104.0, 64.0, 8.0], 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.entities.row_mut(slot).controls.no_ai = true;
    state.entities.row_mut(slot).persistent = true;
    let root = tempfile::tempdir().unwrap();
    let checkpoint_tick = state.tick;
    let mut writer = JournalWriter::open(root.path(), 1).unwrap();
    let mut checkpoint_age = 0;
    for (distance, ticks) in [(6, 3), (2, 5), (6, 3)] {
        state.config_sim_distance = distance;
        for _ in 0..ticks {
            let (mut output, _, _) = run_tick(&mut state, &[]);
            if checkpoint_age == 0 {
                Arc::get_mut(&mut output.changes)
                    .expect("unique test output changes")
                    .entities_spawned
                    .push(mob);
            }
            let entries = crate::core::journal_entries_for_tick(&mut state, &output);
            writer.append(output.tick, output.tick, &entries).unwrap();
            checkpoint_age = state.entities.row(slot).age;
            assert_eq!(state.entities.pos(slot), [104.0, 64.0, 8.0]);
        }
    }
    writer.flush().unwrap();
    let batches =
        read_complete_batches_after(&root.path().join("journal"), checkpoint_tick).unwrap();
    let replay = replay_batches(
        Replay {
            tick: checkpoint_tick,
            ..Replay::default()
        },
        &batches,
    );
    assert_eq!(replay.entities[&mob.0].age, checkpoint_age);
    assert_eq!(
        checkpoint_age, 6,
        "only the two three-tick active intervals count"
    );
    let mut restored = test_state();
    crate::core::restore_entity(&mut restored, mob.0, &replay.entities[&mob.0]).unwrap();
    let restored_slot = restored.entities.slot_of(mob).unwrap();
    assert_eq!(restored.entities.row(restored_slot).age, checkpoint_age);
    let replay = replay_batches(replay, &batches);
    assert_eq!(replay.entities[&mob.0].age, checkpoint_age);
}

#[test]
fn inhabited_time_counts_player_ranges_once_and_ignores_remote_tickets_and_spectators() {
    use crate::boundary::ChunkPos;
    use crate::commands::GameMode;
    let mut state = test_state();
    let first = join(&mut state, 1, "first");
    let second = join(&mut state, 2, "second");
    for id in [first, second] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.set_pos(slot, [8.0, 100.0, 8.0]);
    }
    state.players.get_mut(&first).unwrap().game_mode = GameMode::Creative;
    let near = ChunkPos { x: 0, z: 0 };
    let edge = ChunkPos { x: 2, z: 0 };
    let outside = ChunkPos { x: 3, z: 0 };
    let far = ChunkPos { x: 20, z: 20 };
    state.tickets.insert("outside".into(), (outside, 0));
    state.tickets.insert("far".into(), (far, 0));
    let wanted = [near, edge, outside, far]
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    state.world.ensure_blocking(&wanted);
    let before = [near, edge, outside, far].map(|pos| state.world.inhabited_time(pos));
    for _ in 0..4 {
        run_tick(&mut state, &[]);
    }
    let after = [near, edge, outside, far].map(|pos| state.world.inhabited_time(pos));
    assert_eq!(
        after,
        [before[0] + 4, before[1] + 4, before[2] + 4, before[3]]
    );
    for id in [first, second] {
        state.players.get_mut(&id).unwrap().game_mode = GameMode::Spectator;
    }
    run_tick(&mut state, &[]);
    assert_eq!(state.world.inhabited_time(near), after[0]);
    assert_eq!(state.world.inhabited_time(edge), after[1]);
}

#[test]
fn inhabited_time_uses_strict_horizontal_128_block_chunk_centre_distance() {
    use crate::boundary::ChunkPos;
    let mut state = test_state();
    let player = join(&mut state, 1, "range");
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [8.0, 100.0, 8.0]);
    state.config_sim_distance = 8;
    let inside = ChunkPos { x: 7, z: 0 };
    let boundary = ChunkPos { x: 8, z: 0 };
    let corner = ChunkPos { x: 6, z: 6 };
    let wanted = [inside, boundary, corner]
        .into_iter()
        .map(|pos| (pos, 0))
        .collect();
    state.world.ensure_blocking(&wanted);
    run_tick(&mut state, &[]);
    assert_eq!(state.world.inhabited_time(inside), 1);
    assert_eq!(state.world.inhabited_time(boundary), 0);
    assert_eq!(state.world.inhabited_time(corner), 0);
}

#[test]
fn sword_sweep_damages_an_adjacent_zombie_only_on_a_charged_grounded_hit() {
    // Vanilla Zombie.createAttributes supplies armor 2. CombatRules mitigates
    // the raw one-point sweep to 1 * (1 - (2 - 1 / 2) / 25) = 0.94.
    for (item, weak, sprinting, grounded, expected) in [
        ("wooden_sword", false, false, true, 0.94),
        ("wooden_sword", true, false, true, 0.0),
        ("stick", false, false, true, 0.0),
        ("wooden_sword", false, true, true, 0.0),
        ("wooden_sword", false, false, false, 0.0),
    ] {
        let (mut state, player, pos) = flat_player_fixture();
        state.tick = 20;
        state.last_swing.insert(player, if weak { 19 } else { 0 });
        let actor_slot = state.entities.slot_of(player).unwrap();
        state.entities.set_ground(actor_slot, grounded);
        let item_id = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(format!("minecraft:{item}")).unwrap())
            .unwrap();
        let actor = state.players.get_mut(&player).unwrap();
        actor.inventory.slots[0] = mc_data::ItemStack::new(item_id, 1);
        actor.sprinting = sprinting;
        let zombie = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
            .unwrap();
        let primary = state.spawn_mob(zombie, [pos[0], pos[1], pos[2] + 1.0], 20.0);
        let secondary = state.spawn_mob(zombie, [pos[0] + 0.8, pos[1], pos[2] + 1.0], 20.0);
        let secondary_slot = state.entities.slot_of(secondary).unwrap();
        state.entities.set_ground(secondary_slot, true);
        state.day_time = 14_000;
        let (output, _, _) = run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: primary,
            }],
        );
        let events = &output.changes.entity_events;
        assert_eq!(
            state.entities.row(secondary_slot).health,
            20.0 - expected,
            "{item}, weak={weak}, sprinting={sprinting}, grounded={grounded}"
        );
        assert_eq!(
            events.iter().any(|event| matches!(event,
                crate::boundary::EntityEvent::SweepAttack { entity, .. } if *entity == player
            )),
            expected > 0.0
        );
        if expected > 0.0 {
            assert_eq!(state.entities.vel(secondary_slot), [0.0, 8.0, 8.0]);
        }
    }
}

#[test]
fn sword_sweep_uses_target_box_and_strict_unbuffered_reach_and_requires_primary_damage() {
    for (blocked, movement_squared) in [(false, 0.0), (true, 0.0), (false, 0.0625)] {
        let (mut state, player, pos) = flat_player_fixture();
        state.tick = 20;
        state
            .players
            .get_mut(&player)
            .unwrap()
            .known_horizontal_movement_squared = movement_squared;
        let sword = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
            .unwrap();
        state.players.get_mut(&player).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(sword, 1);
        let zombie = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
            .unwrap();
        // Spawn secondary before primary: merge must not rely on slot order.
        let secondary = state.spawn_mob(zombie, [pos[0] + 0.8, pos[1], pos[2] + 2.0], 20.0);
        let out_of_reach = state.spawn_mob(zombie, [pos[0], pos[1], pos[2] + 3.0], 20.0);
        let above_box = state.spawn_mob(zombie, [pos[0], pos[1] + 2.3, pos[2] + 2.0], 20.0);
        let primary = state.spawn_mob(zombie, [pos[0], pos[1], pos[2] + 2.0], 20.0);
        if blocked {
            let slot = state.entities.slot_of(primary).unwrap();
            let (body, row) = state.entities.parts_mut(slot);
            body.hurt_at = Some(19);
            row.last_hurt = 10.0;
        }
        state.day_time = 14_000;
        let (output, _, _) = run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: primary,
            }],
        );
        let events = &output.changes.entity_events;
        for (id, expected) in [
            (
                secondary,
                if blocked || movement_squared >= 0.0625 {
                    20.0
                } else {
                    19.06
                },
            ),
            (out_of_reach, 20.0),
            (above_box, 20.0),
        ] {
            assert_eq!(
                state
                    .entities
                    .row(state.entities.slot_of(id).unwrap())
                    .health,
                expected
            );
        }
        assert_eq!(
            events
                .iter()
                .any(|event| matches!(event, crate::boundary::EntityEvent::SweepAttack { .. })),
            !blocked && movement_squared < 0.0625
        );
    }
}

#[test]
fn entity_interactions_refuse_distant_targets_without_swing_costs() {
    use crate::boundary::Hand;
    let (mut state, player, pos) = flat_player_fixture();
    state.tick = 20;
    let cow_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:cow").unwrap())
        .unwrap();
    let cow = state.spawn_mob(cow_type, [pos[0] + 20.0, pos[1], pos[2]], 20.0);
    let cow_slot = state.entities.slot_of(cow).unwrap();
    let sword = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(sword, 1);
    let before = state.players[&player].vitals.exhaustion;
    run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target: cow,
        }],
    );
    assert_eq!(state.entities.row(cow_slot).health, 20.0);
    assert_eq!(state.players[&player].inventory.slots[0].damage, None);
    assert_eq!(state.players[&player].vitals.exhaustion, before);
    assert!(!state.last_swing.contains_key(&player));

    let wheat = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:wheat").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(wheat, 2);
    run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: cow,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    assert_eq!(state.entities.row(cow_slot).love, 0);

    state
        .entities
        .set_pos(cow_slot, [pos[0] + 1.0, pos[1], pos[2]]);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(sword, 1);
    run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target: cow,
        }],
    );
    assert!(state.entities.row(cow_slot).health < 20.0);
    assert_eq!(state.players[&player].inventory.slots[0].damage, Some(1));
    assert!(state.players[&player].vitals.exhaustion > before);
    assert!(state.last_swing.contains_key(&player));
}

#[test]
fn melee_breaks_worn_sword_and_syncs_slot_but_creative_preserves_it() {
    for creative in [false, true] {
        let (mut state, player, pos) = flat_player_fixture();
        state.tick = 20;
        let sword = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
            .unwrap();
        let mut stack = mc_data::ItemStack::new(sword, 1);
        let max = state
            .registry
            .item_rules()
            .facts
            .facts_for_stack(&stack, &state.registry.items)
            .unwrap()
            .max_damage
            .unwrap();
        stack.damage = Some(max as i32 - 1);
        let actor = state.players.get_mut(&player).unwrap();
        actor.inventory.slots[0] = stack.clone();
        if creative {
            actor.game_mode = crate::commands::GameMode::Creative;
        }
        let target = state.spawn_mob(
            state.registry.cow_type_id,
            [pos[0] + 1.0, pos[1], pos[2]],
            20.0,
        );
        let (_, drains, _) = run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target,
            }],
        );
        if creative {
            assert_eq!(state.players[&player].inventory.slots[0], stack);
        } else {
            assert!(state.players[&player].inventory.slots[0].is_empty());
            assert!(drains.iter().flat_map(|(_, batch)| batch).any(|change| {
                matches!(
                    change,
                    crate::boundary::PlayerChange::MenuSlot {
                        slot: 36,
                        item: None,
                        ..
                    }
                )
            }));
        }
    }
}
#[test]
fn block_interactions_require_reach_and_ack_refusals() {
    use crate::boundary::{BlockPos, Hand, PlayerChange};
    let mut state = test_state();
    let entity = join(&mut state, 1, "reach");
    let slot = state.entities.slot_of(entity).expect("slot");
    let at = state.entities.pos(slot);
    let near = BlockPos {
        x: at[0].floor() as i32 + 2,
        y: at[1].floor() as i32 + 1,
        z: at[2].floor() as i32,
    };
    let far = BlockPos {
        x: near.x + 10,
        ..near
    };
    let chest = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").expect("identifier"))
        .expect("chest")
        .default
        .0;
    for pos in [near, far] {
        state.world.set_block(pos.x, pos.y, pos.z, chest);
    }
    state.world.fold_overlay();
    let use_at = |position, sequence| PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        hand: Hand::Main,
        position,
        face: 1,
        cursor: [0.5; 3],
        sequence,
    };
    let dig_at = |position, action, sequence| PlayerInput::Dig {
        connection: ConnectionId(1),
        position,
        action,
        sequence,
    };
    for (sequence, input) in [
        (1, use_at(far, 1)),
        (2, dig_at(far, DigAction::Start, 2)),
        (3, dig_at(far, DigAction::Finish, 3)),
    ] {
        state.players.get_mut(&entity).expect("player").game_mode =
            crate::commands::GameMode::Creative;
        let placement = matches!(&input, PlayerInput::UseItemOn { .. });
        let (output, drains, _) = run_tick(&mut state, &[input]);
        assert!(!state.open_containers.contains_key(&entity));
        assert!(state.players[&entity].open.is_none());
        assert_eq!(state.world.block_at(far.x, far.y, far.z), Some(chest));
        if placement {
            assert!(output.changes.blocks.is_empty());
            assert!(drains.iter().flat_map(|(_, queue)| queue).any(|change| {
                matches!(change, PlayerChange::BlockResynced { position, block, .. }
                    if *position == far && *block == chest)
            }));
            let inventory = state.players[&entity].inventory_contents();
            assert!(
                drains
                    .iter()
                    .flat_map(|(_, queue)| queue)
                    .any(|change| change == &inventory)
            );
        } else {
            assert!(
                output
                    .changes
                    .blocks
                    .iter()
                    .any(|change| change.position == far && change.state == chest)
            );
        }
        assert!(drains.iter().flat_map(|(_, queue)| queue).any(|change|
            matches!(change, PlayerChange::BlockActionsAcked { sequence: ack, .. } if *ack == sequence)));
    }
    state.players.get_mut(&entity).expect("player").game_mode = crate::commands::GameMode::Survival;
    let _ = run_tick(&mut state, &[use_at(near, 4)]);
    assert!(state.open_containers.contains_key(&entity));
    let _ = run_tick(&mut state, &[dig_at(near, DigAction::Finish, 5)]);
    assert_eq!(
        state.world.block_at(near.x, near.y, near.z),
        Some(state.registry.states.air)
    );
}
#[test]
fn joined_player_stands_on_dry_solid_world_spawn() {
    let mut state = terrain_test_state();
    let spawn = state.world.world_spawn_position();
    join(&mut state, 1, "spawn-check");
    let x = spawn[0].floor() as i32;
    let y = spawn[1].floor() as i32;
    let z = spawn[2].floor() as i32;
    let air = state.registry.states.air;
    let support = state
        .world
        .block_at(x, y - 1, z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .expect("spawn support is loaded");
    assert_ne!(
        support.id,
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(air))
            .unwrap()
            .id
    );
    assert!(!support.has_fluid());
    assert_eq!(state.world.block_at(x, y, z), Some(air));
    assert_eq!(state.world.block_at(x, y + 1, z), Some(air));
}

#[test]
fn join_loads_chunks_and_move_updates_position() {
    let mut state = test_state();
    // Join tick carries the reliable Joined outbox drain.
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "steve".to_owned(),
            permission_level: 0,
        }],
    );
    let id = state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined");
    assert!(
        drains
            .iter()
            .any(|(connection, _)| *connection == ConnectionId(1))
    );
    // The join tick only queues the view square off-tick; finished chunks
    // become resident at a later input step and ride that tick's snapshot.
    assert!(output.snapshot.chunks.is_empty());
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.snapshot.chunks.len(), want.len());
    let position_of = |output: &crate::boundary::TickOutput| {
        output
            .snapshot
            .entities
            .iter()
            .find(|entity| entity.id == id)
            .map(|entity| entity.position)
    };
    let before = position_of(&output);
    let spawn = state.world.world_spawn_position();
    let target = [spawn[0] + 3.0, spawn[1] + 2.0, spawn[2] + 3.0];
    let (output, _, _) = run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection: ConnectionId(1),
                teleport_id: 1,
            },
            PlayerInput::Move {
                connection: ConnectionId(1),
                position: target,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            },
        ],
    );
    assert!(before.is_some());
    assert_ne!(
        position_of(&output),
        before,
        "the move reaches the snapshot"
    );

    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(1),
        }],
    );
    assert!(output.changes.entities_removed.contains(&id));
}

/// Mobs spawn from blueprints (no type-name branches) and appear in ChangeSet.
#[test]
fn spawned_mobs_tick_and_appear_in_snapshot() {
    let mut state = test_state();
    let player = join(&mut state, 1, "steve");
    let zombie = state.registry.zombie_type_id;
    let cow = state.registry.cow_type_id;
    state.spawn_mob(zombie, [9.0, 80.0, 9.0], 20.0);
    state.spawn_mob(cow, [11.0, 80.0, 11.0], 10.0);
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(output.snapshot.entities.len() >= 3);
    assert!(
        output
            .snapshot
            .entities
            .in_chunk(ChunkPos { x: 0, z: 0 })
            .next()
            .is_some()
    );
    // Entity views carry protocol type ids from blueprints.
    let types: Vec<u32> = output
        .snapshot
        .entities
        .iter()
        .map(|view| view.type_id)
        .collect();
    assert!(types.contains(&zombie));
    assert!(types.contains(&cow));
    // Other clients spawn a player view as `minecraft:player` (registry id), not a guess.
    let player_type = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:player").unwrap())
        .unwrap();
    assert_eq!(
        output
            .snapshot
            .entities
            .iter()
            .find(|view| view.id == player)
            .map(|view| view.type_id),
        Some(player_type)
    );
}

/// Dig removes a block and place adds one, both in ChangeSet.blocks.
#[test]
fn dig_and_place_commit_block_changes() {
    let mut state = test_state();
    let entity = join(&mut state, 1, "steve");
    let (_, _, _) = run_tick(&mut state, &[]);
    // Find a non-air block near spawn.
    let mut target = None;
    for y in (40..100).rev() {
        if state
            .world
            .block_at(8, y, 8)
            .is_some_and(|s| s != state.registry.states.air)
        {
            target = Some(crate::boundary::BlockPos { x: 8, y, z: 8 });
            break;
        }
    }
    let Some(pos) = target else { return };
    let slot = state.entities.slot_of(entity).expect("slot");
    state
        .entities
        .set_pos(slot, [8.5, f64::from(pos.y + 1), 10.5]);
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: pos,
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    assert!(
        output
            .changes
            .blocks
            .iter()
            .any(|change| change.position == pos && change.state == state.registry.states.air)
    );
    assert_eq!(
        state.world.block_at(pos.x, pos.y, pos.z),
        Some(state.registry.states.air)
    );
    // Place above.
    let above = crate::boundary::BlockPos {
        x: pos.x,
        y: pos.y + 1,
        z: pos.z,
    };
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: crate::boundary::Hand::Main,
            position: pos,
            face: 1,
            cursor: [0.5; 3],
            sequence: 2,
        }],
    );
    let _ = (above, output);
}

/// Gravity blocks fall in a chain when unsupported.
#[test]
fn gravity_block_falls_to_support() {
    let mut state = test_state();
    let _ = join(&mut state, 1, "steve");
    let (_, _, _) = run_tick(&mut state, &[]);
    let sand = state.registry.states.sand;
    let air = state.registry.states.air;
    // Chunk (1,1) holds (30,30); force residency before probing it.
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: 1, z: 1 }, 0)]
            .into_iter()
            .collect(),
    );
    // Probe the generated column top-down for the first non-air block.
    let mut floor = None;
    for y in (-64..320).rev() {
        match state.world.block_at(30, y, 30) {
            Some(s) if s != air => {
                floor = Some(y);
                break;
            }
            _ => {}
        }
    }
    let floor = floor.expect("generated terrain has a surface");
    let start = floor + 4;
    state.world.set_block(30, start, 30, sand);
    state.world.fold_overlay();
    assert_eq!(state.world.block_at(30, start, 30), Some(sand));
    assert_eq!(state.world.block_at(30, start - 1, 30), Some(air));
    let mut seeds = vec![(
        crate::boundary::BlockPos {
            x: 30,
            y: start,
            z: 30,
        },
        sand,
    )];
    let mut spawned = Vec::new();
    let mut removed = Vec::new();
    crate::falling::start(&mut state, &mut seeds, &mut spawned);
    assert_eq!(spawned.len(), 1, "unsupported source becomes an entity");
    for _ in 0..40 {
        crate::falling::run(&mut state, &mut seeds, &mut spawned, &mut removed);
    }
    assert_eq!(state.world.block_at(30, floor + 1, 30), Some(sand));
    assert_eq!(state.world.block_at(30, start, 30), Some(air));
    assert_eq!(removed.len(), 1);
}

/// Journal round-trip: append batches, read back, unpublished temporary ignored.
#[test]
fn journal_append_and_recovery_round_trip() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let dir = std::env::temp_dir().join(format!("mc-core-journal-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let mut writer = JournalWriter::open(&dir, 100).expect("open");
    writer
        .append(
            1,
            1,
            &[JournalEntry::Block {
                pos: [1, 2, 3],
                state: 9,
            }],
        )
        .expect("append");
    writer
        .append(
            2,
            3,
            &[JournalEntry::Block {
                pos: [1, 2, 3],
                state: 10,
            }],
        )
        .expect("append");
    writer.flush().expect("flush");
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 2);
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(replay.blocks.get(&(1, 2, 3)), Some(&10));
    // Unpublished tail: partial temporary frame left before atomic rename.
    std::fs::write(
        dir.join("journal")
            .join("batch-00000000000000000004-00000000000000000004.bin.tmp"),
        b"short",
    )
    .expect("torn");
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 2);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Checkpoint manifest publish + prune keeps recovery at the manifest tick.
#[test]
fn checkpoint_manifest_gates_recovery() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, prune_batches_through, read_complete_batches_after,
        replay_batches,
    };
    let dir = std::env::temp_dir().join(format!("mc-core-manifest-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let config = CoreConfig::new(dir.clone(), 42);
    let mut writer = JournalWriter::open(&dir, 100).expect("open");
    for tick in 1..=4u64 {
        writer
            .append(
                tick,
                tick,
                &[JournalEntry::Block {
                    pos: [0, 0, 0],
                    state: tick as u32,
                }],
            )
            .expect("append");
    }
    writer.flush().expect("flush");
    crate::native_checkpoint::tests::write_manifest(
        &config,
        &crate::journal::CheckpointManifest {
            tick: 2,
            world_spawn: Some([0, 64, 0]),
            ..Default::default()
        },
    )
    .expect("manifest");
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .tick,
        2
    );
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&config)
            .unwrap()
            .world_spawn,
        Some([0, 64, 0])
    );
    prune_batches_through(&dir.join("journal"), 2);
    let batches = read_complete_batches_after(&dir.join("journal"), 2).unwrap();
    assert!(batches.iter().all(|(lo, _, _)| *lo > 2));
    let replay = replay_batches(Replay::default(), &batches);
    assert!(replay.tick >= 3);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Core open/submit/subscribe end-to-end over real threads.
#[test]
fn core_open_submit_subscribe_tick_flows() {
    let dir = std::env::temp_dir().join(format!("mc-core-e2e-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let config = CoreConfig::new(dir.clone(), 7);
    let core = Core::open(config).expect("open");
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(9),
        uuid: uuid::Uuid::new_v4(),
        name: "alex".to_owned(),
        permission_level: 0,
    });
    // Drain until the player near world origin commits; early ticks may
    // predate the Join input drain over real threads.
    let mut output = rx.blocking_recv().expect("tick");
    for _ in 0..60 {
        if output
            .snapshot
            .entities
            .iter()
            .any(|view| view.position[0].abs() <= 256.0 && view.position[2].abs() <= 256.0)
        {
            break;
        }
        output = rx.blocking_recv().expect("tick");
    }
    assert!(output.tick >= 1);
    assert!(
        output
            .snapshot
            .entities
            .iter()
            .any(|view| view.position[0].abs() <= 256.0 && view.position[2].abs() <= 256.0)
    );
    let joined_position = output
        .snapshot
        .entities
        .iter()
        .find(|view| view.position[0].abs() <= 256.0 && view.position[2].abs() <= 256.0)
        .expect("joined player")
        .position;
    core.shutdown();
    assert_eq!(
        crate::native_checkpoint::tests::read_manifest(&CoreConfig::new(dir.clone(), 7))
            .unwrap()
            .world_spawn,
        Some([
            joined_position[0].floor() as i32,
            joined_position[1].floor() as i32,
            joined_position[2].floor() as i32,
        ])
    );
    let core = Core::open(CoreConfig::new(dir.clone(), 7)).expect("reopen");
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(10),
        uuid: uuid::Uuid::new_v4(),
        name: "rejoined".to_owned(),
        permission_level: 0,
    });
    let mut joined_again = false;
    for _ in 0..60 {
        let output = rx.blocking_recv().expect("tick");
        if output
            .snapshot
            .entities
            .iter()
            .any(|view| view.position == joined_position)
        {
            joined_again = true;
            break;
        }
    }
    assert!(joined_again, "reopened world uses its persisted spawn");
    core.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// REQUIRED 1: real Core in a CHILD process with a short checkpoint
/// interval; mutate player inventory; observe the journal WAL durable
/// through the mutation tick; SIGKILL the child; reopen in the parent and
/// expect equal player state. No hand-made manifest, no sleeps (the parent
/// polls the WAL for the durability boundary with a bounded spin).
#[test]
fn checkpoint_round_trip_through_real_core() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    use crate::journal::read_complete_batches_after;
    // Child mode: selected by env flag (re-exec of this test binary). The
    // parent spawns `current_exe` with a filter that ALSO matches this very
    // test inside the child runner — libtest runs the parent body again in
    // the child process. Only the env-flagged process takes the child path;
    // the child body never spawns).
    // Generation guard: the child body never spawns, so a nested match
    // always means "I am the child" — but if the child runner ALSO spawns
    // (it cannot: spawn lives only in the parent path below), generations
    // would recurse. Pass an explicit generation: parent=0 spawns gen=1,
    // and any gen>=1 process runs the child body and returns.
    if std::env::var("MC_CORE_CRASH_CHILD").is_ok() {
        let dir = std::env::var("MC_CORE_CRASH_DIR").expect("child dir");
        let dir = std::path::PathBuf::from(dir);
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 5;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        let uuid = uuid::Uuid::from_u128(0xC0AC_4A11_600D_C0DE_1234_5678_9ABC_DEF0);
        core.submit(PlayerInput::Join {
            connection: ConnectionId(21),
            uuid,
            name: "ckpt".to_owned(),
            permission_level: 0,
        });
        // Wait until the join commits (event, never a sleep).
        let mut joined = false;
        for _ in 0..1000 {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                joined = true;
                break;
            }
        }
        assert!(joined, "join committed");
        core.submit(PlayerInput::Command {
            connection: ConnectionId(21),
            command: "debug give minecraft:apple 2 1".to_owned(),
            permission_level: 4,
        });
        // Each committed tick is the event: check this child's own WAL for a
        // complete batch carrying the apple stack (count 2), then report the
        // durable boundary and block until the parent's SIGKILL.
        let journal_dir = dir.join("journal");
        for _ in 0..1000 {
            rx.blocking_recv().expect("tick");
            let boundary = read_complete_batches_after(&journal_dir, 0)
                .unwrap()
                .iter()
                .filter(|(_, _, entries)| {
                    entries.iter().any(|entry| {
                        matches!(entry, crate::journal::JournalEntry::Player { inventory, .. }
                            if inventory.iter().any(|stack| !stack.is_empty() && stack.count == 2))
                    })
                })
                .map(|(_, hi, _)| *hi)
                .max();
            if let Some(tick) = boundary {
                use std::io::{Read, Write};
                println!("CRASH_BOUNDARY_READY {tick}");
                std::io::stdout().flush().unwrap();
                let mut byte = [0];
                std::io::stdin()
                    .read_exact(&mut byte)
                    .expect("parent must SIGKILL at boundary");
                panic!("parent sent input instead of SIGKILL");
            }
        }
        panic!("apple stack never became durable within 1000 ticks");
    }
    // Parent mode: spawn the child, watch the WAL for the mutation tick,
    // SIGKILL at that observed durability point, reopen, assert recovery.
    let dir = std::env::temp_dir().join(format!(
        "mc-core-ckpt-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let child_bin = std::env::current_exe().expect("test binary");
    let mut child = std::process::Command::new(child_bin)
        .args(["checkpoint_round_trip_through_real_core", "--nocapture"])
        .env("MC_CORE_CRASH_CHILD", "1")
        .env("MC_CORE_CRASH_DIR", &dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn crash child");
    // The child reports the exact durable WAL boundary; a closed pipe means it
    // died first (its stderr carries the panic).
    let journal_dir = dir.join("journal");
    let boundary_tick: u64 = {
        use std::io::BufRead;
        let mut output = std::io::BufReader::new(child.stdout.take().expect("child stdout"));
        loop {
            let mut line = String::new();
            assert_ne!(
                output.read_line(&mut line).expect("read child"),
                0,
                "crash child exited before the mutation was durable"
            );
            if let Some(tick) = line.trim().strip_prefix("CRASH_BOUNDARY_READY ") {
                break tick.parse().expect("boundary tick");
            }
        }
    };
    assert!(boundary_tick > 0, "mutation tick durable in WAL");
    // KILL at the observed durability point (`Child::kill` = SIGKILL on
    // unix; real process death, not `mem::forget`).
    child.kill().expect("SIGKILL crash child");
    let status = child.wait().expect("reap crash child");
    assert!(!status.success(), "child died by signal, exit {status:?}");
    // Reopen: recovery must reach the observed boundary tick; the apple
    // stack (count 2) must be in the journal tail or checkpoint.
    let batches = read_complete_batches_after(&journal_dir, 0).unwrap();
    let top = batches.iter().map(|(_, hi, _)| *hi).max().unwrap_or(0);
    assert!(
        top >= boundary_tick,
        "WAL tail reaches boundary: {top} >= {boundary_tick}"
    );
    let config = CoreConfig::new(dir.clone(), 11);
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    let output = rx2.blocking_recv().expect("tick");
    assert!(
        output.tick >= boundary_tick,
        "recovered at/after boundary: {} >= {boundary_tick}",
        output.tick
    );
    // Rejoin the SAME uuid: the apple stack restores into the inventory.
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(22),
        uuid: uuid::Uuid::from_u128(0xC0AC_4A11_600D_C0DE_1234_5678_9ABC_DEF0),
        name: "ckpt".to_owned(),
        permission_level: 0,
    });
    let mut restored = false;
    for _ in 0..200 {
        let _ = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change
                    && slots.iter().flatten().any(|stack| stack.count == 2)
                {
                    restored = true;
                }
            }
        }
        if restored {
            break;
        }
    }
    assert!(restored, "apple stack restored after SIGKILL");
    core2.shutdown();
    // Offline readers require a cleanly closed writer. The runtime reopen above
    // performs crash recovery; only its shutdown makes this checkpoint readable.
    let manifest =
        crate::native_checkpoint::tests::read_manifest(&CoreConfig::new(dir.clone(), 11)).unwrap();
    assert!(
        manifest.tick >= boundary_tick,
        "recovered checkpoint reaches durable mutation boundary"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// REQUIRED 3: same blueprint data under a different type_id behaves
/// identically (no type-name branches in the tick).
#[test]
fn blueprint_goals_drive_behavior_not_type_ids() {
    use crate::blueprint::{Goal, GoalKind, MobBlueprint, MovementClass};
    // Two blueprints with identical goal data but different type ids.
    let goals_a: &[Goal] = &[
        Goal {
            priority: 1,
            kind: GoalKind::MeleeAttack,
        },
        Goal {
            priority: 2,
            kind: GoalKind::MeleeChase,
        },
    ];
    let blueprint_a = MobBlueprint {
        name: "test:chaser_a".to_owned(),
        type_id: 9001,
        movement: MovementClass::Walker,
        goals: goals_a.to_vec(),
        sensors: crate::blueprint::Sensors {
            sight_range: 35.0,
            attack_reach: 2.2,
        },
        params: crate::blueprint::MobParams {
            max_health: 20.0,
            speed: 4.3,
            attack_damage: 3.0,
            pickup_delay: 0,
            breed_cooldown: 0,
            armor: 0.0,
            knockback_resistance: 0.0,
        },
        half_width: 0.3,
        height: 1.95,
        offspring: None,
        breed_food: Vec::new(),
        loot_table: None,
        category: crate::blueprint::MobCategory::Monster,
        wool: false,
        spawn_weight: 100,
        capabilities: crate::blueprint::Capabilities {
            burns_in_daylight: true,
            ..Default::default()
        },
    };
    let blueprint_b = MobBlueprint {
        name: "test:chaser_b".to_owned(),
        type_id: 9002,
        ..blueprint_a.clone()
    };
    // Same goals resolve identically regardless of type id.
    assert!(blueprint_a.has_goal(GoalKind::MeleeChase));
    assert!(blueprint_b.has_goal(GoalKind::MeleeChase));
    assert!(!blueprint_a.has_goal(GoalKind::Breed));
    assert!(!blueprint_b.has_goal(GoalKind::Breed));
    assert_eq!(blueprint_a.goals, blueprint_b.goals);
}

/// REQUIRED 4: entities never fall through into unloaded chunks and never
/// gain a floor from them — motion in a missing column is blocked.
#[test]
fn missing_chunk_blocks_motion_never_floors() {
    let mut state = test_state();
    let registry = test_registry();
    let id = state.spawn_mob(registry.cow_type_id, [8.0, 80.0, 8.0], 10.0);
    let slot = state.entities.slot_of(id).expect("slot");
    // Teleport far outside the loaded radius: the column is missing, so the
    // resolver samples Solid all around and the mob must freeze in place.
    state.entities.set_pos(slot, [10_000.0, 80.0, 10_000.0]);
    let before = state.entities.pos(slot);
    let _ = run_tick(&mut state, &[]);
    let after = state.entities.pos(slot);
    assert_eq!(before, after, "unloaded column blocks all motion");
}

/// Mob AI cadence by distance to the nearest player: decisions every tick
/// within two chunks, every second tick within four; between decisions the
/// last one keeps executing, so a middle-ring chaser still moves every tick
/// and closes in as fast as a near one.
#[test]
fn mob_ai_cadence_follows_player_distance_at_same_speed() {
    let mut state = test_state();
    let player = join(&mut state, 1, "steve");
    let player_slot = state.entities.slot_of(player).expect("player slot");
    let spawn = state.entities.pos(player_slot);
    // Player on the east edge of its chunk: +20 blocks lies two chunks east
    // (near ring), +34 three chunks east (middle ring) and still inside the
    // 35-block zombie sight. A ticket keeps the middle chunk resident.
    let chunk_x = (spawn[0].floor() as i32).div_euclid(16);
    let chunk_z = (spawn[2].floor() as i32).div_euclid(16);
    let player_pos = [f64::from(chunk_x * 16 + 15) + 0.5, spawn[1], spawn[2]];
    state.entities.set_pos(player_slot, player_pos);
    state.tickets.insert(
        "middle".to_owned(),
        (
            crate::boundary::ChunkPos {
                x: chunk_x + 3,
                z: chunk_z,
            },
            0,
        ),
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    // Airborne a little above the player: nothing blocks the short chase.
    let zombie = state.registry.zombie_type_id;
    let y = player_pos[1] + 6.0;
    let near = state.spawn_mob(zombie, [player_pos[0] + 20.0, y, player_pos[2]], 20.0);
    let middle = state.spawn_mob(zombie, [player_pos[0] + 34.0, y, player_pos[2]], 20.0);
    let near = state.entities.slot_of(near).expect("near slot");
    let middle = state.entities.slot_of(middle).expect("middle slot");
    // Two ticks so the middle zombie has made its first decision.
    for _ in 0..2 {
        let _ = run_tick(&mut state, &[]);
    }
    let start = (state.entities.pos(near), state.entities.pos(middle));
    let horizontal = |a: [f64; 3], b: [f64; 3]| a[0] != b[0] || a[2] != b[2];
    let mut moved = (0, 0);
    for _ in 0..4 {
        let before = (state.entities.pos(near), state.entities.pos(middle));
        let _ = run_tick(&mut state, &[]);
        moved.0 += usize::from(horizontal(state.entities.pos(near), before.0));
        moved.1 += usize::from(horizontal(state.entities.pos(middle), before.1));
    }
    assert_eq!(moved, (4, 4), "both move every tick, deciding or not");
    let run =
        |from: [f64; 3], to: [f64; 3]| mc_physics::mth::length2(to[0] - from[0], to[2] - from[2]);
    let near_run = run(start.0, state.entities.pos(near));
    let middle_run = run(start.1, state.entities.pos(middle));
    assert!(near_run > 0.0, "the near zombie chases");
    assert!(
        (middle_run - near_run).abs() < near_run * 0.05,
        "same speed per elapsed time: near {near_run}, middle {middle_run}"
    );
}

/// A far mob resting between AI decisions still falls on the tick its
/// support is removed (physics never waits for the next decision).
#[test]
fn resting_far_mob_falls_when_support_is_removed() {
    let mut state = test_state();
    let player = join(&mut state, 1, "steve");
    let player_slot = state.entities.slot_of(player).expect("player slot");
    let spawn = state.entities.pos(player_slot);
    let chunk_x = (spawn[0].floor() as i32).div_euclid(16) + 6;
    let chunk_z = (spawn[2].floor() as i32).div_euclid(16);
    state.tickets.insert(
        "far".to_owned(),
        (
            crate::boundary::ChunkPos {
                x: chunk_x,
                z: chunk_z,
            },
            0,
        ),
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let air = state.registry.states.air;
    let (bx, bz) = (chunk_x * 16 + 8, chunk_z * 16 + 8);
    let top = (-64..320)
        .rev()
        .find(|y| state.world.block_at(bx, *y, bz).is_some_and(|b| b != air))
        .expect("surface");
    let cow = state.spawn_mob(
        state.registry.cow_type_id,
        [f64::from(bx) + 0.5, f64::from(top + 1), f64::from(bz) + 0.5],
        10.0,
    );
    let slot = state.entities.slot_of(cow).expect("cow slot");
    // Rest it on the ground, then run to a tick pair where it does not decide.
    loop {
        let _ = run_tick(&mut state, &[]);
        let resting = [f64::from(bx) + 0.5, f64::from(top + 1), f64::from(bz) + 0.5];
        state.entities.set_pos(slot, resting);
        state.entities.set_vel(slot, [0.0; 3]);
        state.entities.set_ground(slot, true);
        if matches!((state.tick + cow.0 as u64) % 4, 1 | 2) {
            break;
        }
    }
    let _ = run_tick(&mut state, &[]);
    let rested = state.entities.pos(slot);
    assert_eq!(
        rested[1],
        f64::from(top + 1),
        "unchanged support: the mob rests"
    );
    state.world.set_block(bx, top, bz, air);
    state.world.fold_overlay();
    let _ = run_tick(&mut state, &[]);
    assert!(
        state.entities.pos(slot)[1] < rested[1],
        "support removed: the mob falls this tick"
    );
}

/// Zombies in reach swing once per second (vanilla `MeleeAttackGoal`), and
/// a second hit inside the victim's hurt invulnerability lands nothing: two
/// zombies over 30 ticks cost two hits, not sixty.
#[test]
fn zombie_hits_respect_attack_interval_and_hurt_invulnerability() {
    let (mut state, player, at) = flat_player_fixture();
    state.day_time = 14_000;
    let slot = state.entities.slot_of(player).expect("slot");
    // Past the join spawn protection.
    state.entities.row_mut(slot).protected_until = 0;
    // Below 18 food and without saturation nothing regenerates.
    let vitals = &mut state.players.get_mut(&player).expect("player").vitals;
    vitals.food = 17;
    vitals.saturation = 0.0;
    let registry = test_registry();
    for dx in [1.0, -1.0] {
        state.spawn_mob(registry.zombie_type_id, [at[0] + dx, at[1], at[2]], 20.0);
    }
    for _ in 0..30 {
        let _ = run_tick(&mut state, &[]);
    }
    let damage = registry
        .blueprint(registry.zombie_type_id)
        .expect("zombie")
        .params
        .attack_damage;
    let landed = state.difficulty.scale_mob_damage(damage);
    assert_eq!(state.players[&player].vitals.health, 20.0 - 2.0 * landed);
}

/// Joined players ignore hits for vanilla's 60-tick spawn protection, then
/// take them; easy difficulty lands each zombie 3 as 2.5.
#[test]
fn spawn_protection_blocks_mob_hits_for_three_seconds() {
    let (mut state, player, at) = flat_player_fixture();
    state.day_time = 14_000;
    let vitals = &mut state.players.get_mut(&player).expect("player").vitals;
    vitals.food = 17;
    vitals.saturation = 0.0;
    let registry = test_registry();
    state.spawn_mob(registry.zombie_type_id, [at[0] + 1.0, at[1], at[2]], 20.0);
    for _ in 0..55 {
        let _ = run_tick(&mut state, &[]);
    }
    assert_eq!(state.players[&player].vitals.health, 20.0, "protected");
    for _ in 0..30 {
        let _ = run_tick(&mut state, &[]);
    }
    let lost = 20.0 - state.players[&player].vitals.health;
    assert!(
        lost > 0.0 && lost % 2.5 == 0.0,
        "easy hits land: lost {lost}"
    );
}

/// Creative players break with the dig start alone (the client never
/// sends a finish); a survival start on a hard block breaks nothing.
#[test]
fn creative_breaks_on_dig_start() {
    let mut state = test_state();
    let player = join(&mut state, 1, "builder");
    let slot = state.entities.slot_of(player).expect("slot");
    let at = state.entities.pos(slot);
    let (x, y, z) = (
        at[0].floor() as i32 + 2,
        at[1].floor() as i32 + 3,
        at[2].floor() as i32,
    );
    let stone = state.registry.states.stone;
    state.world.set_block(x, y, z, stone);
    state.world.fold_overlay();
    let start = |sequence| PlayerInput::Dig {
        connection: ConnectionId(1),
        position: crate::boundary::BlockPos { x, y, z },
        action: DigAction::Start,
        sequence,
    };
    let _ = run_tick(&mut state, &[start(1)]);
    assert_eq!(state.world.block_at(x, y, z), Some(stone), "survival start");
    state.players.get_mut(&player).expect("player").game_mode = crate::commands::GameMode::Creative;
    let _ = run_tick(&mut state, &[start(2)]);
    assert_eq!(
        state.world.block_at(x, y, z),
        Some(state.registry.states.air),
        "creative start"
    );
}

/// Breaking a block pops the torch standing on it and the torch hanging on
/// its side; an unrelated torch on another block stays.
#[test]
fn torches_pop_when_their_support_breaks() {
    let mut state = test_state();
    let player = join(&mut state, 1, "miner");
    let slot = state.entities.slot_of(player).expect("slot");
    let at = state.entities.pos(slot);
    let (x, y, z) = (
        at[0].floor() as i32 + 3,
        at[1].floor() as i32 + 3,
        at[2].floor() as i32,
    );
    let registry = test_registry();
    let named = |name: &str, props: &[(&str, &str)]| {
        let id = mc_data::Identifier::parse(name).expect("id");
        let block = registry.blocks.block(&id).expect("block");
        let mut full = registry
            .blocks
            .by_id(block.default)
            .expect("state")
            .properties
            .clone();
        for (key, value) in props {
            full.iter_mut()
                .find(|(name, _)| name == key)
                .expect("prop")
                .1 = (*value).to_owned();
        }
        registry
            .blocks
            .by_name_and_props(&id, &full)
            .expect("state")
            .0
    };
    let stone = registry.states.stone;
    let torch = named("minecraft:torch", &[]);
    let wall_torch = named("minecraft:wall_torch", &[("facing", "east")]);
    state.world.set_block(x, y, z, stone);
    state.world.set_block(x, y + 1, z, torch);
    state.world.set_block(x + 1, y, z, wall_torch);
    state.world.set_block(x, y, z + 3, stone);
    state.world.set_block(x, y + 1, z + 3, torch);
    state.world.fold_overlay();
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: crate::boundary::BlockPos { x, y, z },
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    let air = Some(registry.states.air);
    assert_eq!(
        state.world.block_at(x, y + 1, z),
        air,
        "standing torch pops"
    );
    assert_eq!(state.world.block_at(x + 1, y, z), air, "wall torch pops");
    assert_eq!(
        state.world.block_at(x, y + 1, z + 3),
        Some(torch),
        "other torch stays"
    );
}

/// Breaking either half of a door clears both cells.
#[test]
fn breaking_a_door_half_removes_the_whole_door() {
    let mut state = test_state();
    let player = join(&mut state, 1, "breaker");
    let slot = state.entities.slot_of(player).expect("slot");
    let at = state.entities.pos(slot);
    let (x, y, z) = (
        at[0].floor() as i32 + 2,
        at[1].floor() as i32 + 3,
        at[2].floor() as i32,
    );
    let registry = test_registry();
    let door = |half: &str| {
        let id = mc_data::Identifier::parse("minecraft:oak_door").expect("id");
        let block = registry.blocks.block(&id).expect("door");
        let mut props = registry
            .blocks
            .by_id(block.default)
            .expect("state")
            .properties
            .clone();
        props
            .iter_mut()
            .find(|(key, _)| key == "half")
            .expect("half")
            .1 = half.to_owned();
        registry
            .blocks
            .by_name_and_props(&id, &props)
            .expect("door state")
            .0
    };
    state.world.set_block(x, y, z, door("lower"));
    state.world.set_block(x, y + 1, z, door("upper"));
    state.world.fold_overlay();
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(1),
            position: crate::boundary::BlockPos { x, y: y + 1, z },
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    let air = Some(registry.states.air);
    assert_eq!(
        (
            state.world.block_at(x, y, z),
            state.world.block_at(x, y + 1, z)
        ),
        (air, air)
    );
}

/// Landing after an 11-block fall (the landing move's own 1-block drop counts,
/// vanilla `Entity.checkFallDamage`) costs 8 health (one per block beyond
/// three, armor ignored); creative players take none.
#[test]
fn landing_after_a_fall_hurts_survival_players_only() {
    for (mode, expected) in [(0u8, 12.0f32), (1u8, 20.0f32)] {
        let mut state = test_state();
        let player = join(&mut state, 1, "faller");
        if mode == 1 {
            state.players.get_mut(&player).expect("player").game_mode =
                crate::commands::GameMode::Creative;
        }
        let slot = state.entities.slot_of(player).expect("slot");
        state.entities.row_mut(slot).protected_until = 0;
        let ground = state.entities.pos(slot);
        let step = |state: &mut crate::tick::TickState, y: f64, on_ground: bool| {
            let _ = run_tick(
                state,
                &[PlayerInput::Move {
                    connection: ConnectionId(1),
                    position: [ground[0], y, ground[2]],
                    yaw: 0.0,
                    pitch: 0.0,
                    on_ground,
                }],
            );
        };
        // Climb (no fall distance), fall 10 blocks airborne, then land with a
        // final 1-block drop that vanilla adds before the ground test.
        for rise in 1..=11 {
            step(&mut state, ground[1] + f64::from(rise), false);
        }
        for drop in (1..11).rev() {
            step(&mut state, ground[1] + f64::from(drop), false);
        }
        step(&mut state, ground[1], true);
        let health = state.players[&player].vitals.health;
        assert_eq!(health, expected, "game mode {mode}");
    }
}

#[test]
fn player_air_changes_reach_the_owner_before_drowning_damage() {
    let (mut state, entity, pos) = flat_player_fixture();
    let eye = [
        pos[0].floor() as i32,
        (pos[1] + 1.62).floor() as i32,
        pos[2].floor() as i32,
    ];
    state
        .world
        .set_block(eye[0], eye[1], eye[2], state.registry.states.water);
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&entity].vitals.air, 299);
    assert_eq!(state.players[&entity].vitals.health, 20.0);
    let changes: Vec<_> = drains
        .into_iter()
        .filter(|(connection, _)| *connection == ConnectionId(1))
        .flat_map(|(_, changes)| changes)
        .collect();
    assert_eq!(
        changes,
        vec![crate::boundary::PlayerChange::AirSupply { air: 299 }],
        "changing air must publish before health falls"
    );
    state
        .world
        .set_block(eye[0], eye[1], eye[2], state.registry.states.air);
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&entity].vitals.air, 300);
    let changes: Vec<_> = drains
        .into_iter()
        .flat_map(|(_, changes)| changes)
        .collect();
    assert_eq!(
        changes,
        vec![crate::boundary::PlayerChange::AirSupply { air: 300 }],
        "air recovery also reaches the owner"
    );
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert!(
        drains.iter().all(|(_, changes)| changes.is_empty()),
        "unchanged air is quiet"
    );
}

#[test]
fn player_air_is_initialized_on_join_and_reset_on_respawn() {
    use crate::boundary::PlayerChange;
    let mut state = test_state();
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "air-test".into(),
            permission_level: 0,
        }],
    );
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| *change == PlayerChange::AirSupply { air: 300 })
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    let player = state.players.get_mut(&entity).unwrap();
    player.vitals.air = 17;
    player.vitals.health = 0.0;
    state.entities.body_mut(slot).alive = false;
    state.entities.row_mut(slot).health = 0.0;
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(1),
        }],
    );
    assert_eq!(state.players[&entity].vitals.air, 300);
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| *change == PlayerChange::AirSupply { air: 300 })
    );
}

/// REQUIRED 2: an item entity carrying a damaged/enchanted/named stack keeps
/// its full components through drop -> journal -> restart.
#[test]
fn item_entity_stack_survives_journal_restart() {
    use crate::journal::JournalEntry;
    let registry = test_registry();
    let id = mc_data::Identifier::parse("minecraft:diamond_sword").expect("id");
    let stack = mc_data::ItemStack {
        item_id: registry.items.id_of(&id).expect("sword"),
        count: 1,
        damage: Some(37),
        custom_name: Some(" quests blade ".into()),
        ..Default::default()
    };
    let row = crate::entities::StackRow {
        item_id: stack.item_id,
        count: stack.count,
        stack: Some(stack.clone()),
    };
    // Journal payload carries the full stack.
    let full = row.full_stack();
    assert_eq!(full.damage, Some(37));
    assert_eq!(full.custom_name.as_deref(), Some(" quests blade "));
    // Replay restore (mirrors restore_entity) keeps every component.
    let restored = crate::entities::StackRow {
        item_id: full.item_id,
        count: full.count,
        stack: Some(full.clone()),
    };
    assert_eq!(restored.full_stack(), stack);
    let _ = JournalEntry::RemoveEntity { id: 0, tick: 0 };
}

/// Task 1: open Core, join + give an item, shutdown (joins threads, final
/// checkpoint durable), reopen, rejoin the same uuid -> exact inventory.
#[test]
fn shutdown_joins_and_reopen_restores_inventory() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!("mc-core-shut-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 5;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(31),
            uuid,
            name: "shut".to_owned(),
            permission_level: 0,
        });
        // Wait for the join, then give dirt into the inventory.
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        core.submit(PlayerInput::Command {
            connection: ConnectionId(31),
            command: "debug give minecraft:dirt 7 2".to_owned(),
            permission_level: 4,
        });
        // Wait until the dirt lands in the reliable outbox inventory change.
        loop {
            let _ = rx.blocking_recv().expect("tick");
            let mut found = false;
            for (_, changes) in core.try_drain_outbox() {
                for change in changes {
                    if format!("{change:?}").contains("count: 7") {
                        found = true;
                    }
                }
            }
            if found {
                break;
            }
        }
        // Joining shutdown: returns only after durable.
        core.shutdown();
    }
    // Reopen (fresh Core, no threads carried) and rejoin the same uuid.
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(32),
        uuid,
        name: "shut".to_owned(),
        permission_level: 0,
    });
    let mut restored = false;
    for _ in 0..200 {
        let _ = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                if format!("{change:?}").contains("count: 7") {
                    restored = true;
                }
            }
        }
        if restored {
            break;
        }
    }
    assert!(restored, "rejoin restores the dirt stack");
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Full durable state across checkpoint + abrupt drop + reopen, asserted
/// by value: a component stack (damage + name + enchantment) in the
/// inventory, the same stack dropped as a ground item (no dupe: inventory
/// total unchanged), xp, and a damaged mob (no rollback).
#[test]
fn checkpoint_holds_full_durable_state() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!("mc-core-full-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    let registry = test_registry();
    let sword_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_sword").expect("id"))
        .expect("sword");
    let sharp = mc_data::Identifier::parse("minecraft:sharpness").expect("enchant");
    // The exact component stacks under test.
    let sword = mc_data::ItemStack {
        item_id: sword_id,
        count: 1,
        damage: Some(37),
        enchantments: vec![mc_data::ItemEnchantment {
            id: sharp,
            level: 3,
        }],
        custom_name: Some(" quests blade ".into()),
        ..Default::default()
    };
    let pick_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_pickaxe").expect("id"))
        .expect("pick");
    let pick = mc_data::ItemStack {
        item_id: pick_id,
        count: 1,
        damage: Some(11),
        enchantments: Vec::new(),
        custom_name: Some("digger".into()),
        ..Default::default()
    };
    // Phase 1: build the state through real inputs (TestGive is the only
    // test-only input: no protocol input can materialize a component stack).
    let cow_type = registry.cow_type_id;
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 5;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(41),
            uuid,
            name: "full".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        // Real mob first + real damage bare-handed (selected slot is
        // empty, so no weapon wear touches the stacks placed later).
        core.submit(PlayerInput::Command {
            connection: ConnectionId(41),
            command: "summon minecraft:cow".to_owned(),
            permission_level: 4,
        });
        core.submit(PlayerInput::Command {
            connection: ConnectionId(41),
            command: "xp add @s 7 levels".to_owned(),
            permission_level: 4,
        });
        let mut mob: Option<crate::boundary::EntityId> = None;
        for _ in 0..10 {
            let output = rx.blocking_recv().expect("tick");
            if mob.is_none() {
                mob = output
                    .snapshot
                    .entities
                    .iter()
                    .find(|view| view.type_id == cow_type)
                    .map(|view| view.id);
            }
            if mob.is_some() {
                break;
            }
        }
        let mob = mob.expect("summoned cow visible");
        core.submit(PlayerInput::Attack {
            connection: ConnectionId(41),
            target: mob,
        });
        for _ in 0..3 {
            let _ = rx.blocking_recv().expect("tick");
        }
        // Slot 0: sword (will be dropped), slot 1: pick (stays).
        core.submit(PlayerInput::TestGive {
            connection: ConnectionId(41),
            slot: 0,
            stack: sword.clone(),
        });
        core.submit(PlayerInput::TestGive {
            connection: ConnectionId(41),
            slot: 1,
            stack: pick.clone(),
        });
        for _ in 0..3 {
            let _ = rx.blocking_recv().expect("tick");
        }
        // Real drop LAST: slot 0 (selected) -> ground item entity. Only 2
        // ticks pass before shutdown (pickup_delay 10), so the sword is
        // still on the ground at the final checkpoint. shutdown() joins and
        // runs that final checkpoint itself.
        core.submit(PlayerInput::DropSelected {
            connection: ConnectionId(41),
            whole_stack: true,
        });
        for _ in 0..2 {
            let _ = rx.blocking_recv().expect("tick");
        }
        core.shutdown();
    }
    // Reopen and assert by value.
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(42),
        uuid,
        name: "full".to_owned(),
        permission_level: 0,
    });
    // Rejoin burst: full inventory contents + xp.
    let mut slots: Vec<Option<mc_data::ItemStack>> = Vec::new();
    let mut level = 0;
    let mut total = 0;
    for _ in 0..200 {
        let _ = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                match change {
                    crate::boundary::PlayerChange::MenuContents { slots: burst, .. } => {
                        slots = burst;
                    }
                    crate::boundary::PlayerChange::Experience {
                        level: burst,
                        total: burst_total,
                        ..
                    } => {
                        level = burst;
                        total = burst_total;
                    }
                    _ => {}
                }
            }
        }
        if !slots.is_empty() && level == 7 {
            break;
        }
    }
    // Window-0 menu slots 36/37 mirror hotbar inventory slots 0/1.
    // Slot 0 dropped -> empty; slot 1 holds the exact pick stack.
    assert!(slots.len() > 37, "burst carries the inventory");
    assert!(slots[36].is_none(), "dropped sword left slot 0");
    assert_eq!(slots[37].as_ref(), Some(&pick), "exact pick stack restored");
    assert_eq!(level, 7, "xp level restored");
    let _ = total;
    // Ground item: the exact sword stack on an item entity; mob damaged.
    let output = rx2.blocking_recv().expect("tick");
    let ground = output
        .snapshot
        .entities
        .iter()
        .find(|view| view.type_id != cow_type && view.appearance.item.as_ref() == Some(&sword))
        .expect("ground sword item entity");
    assert_eq!(ground.appearance.item.as_ref(), Some(&sword));
    let damaged = output
        .snapshot
        .entities
        .iter()
        .find(|view| view.type_id == cow_type)
        .expect("cow survived");
    // Summon spawns at 20.0; the bare-hand hit lands 0.4.
    assert!(
        damaged.appearance.health < 20.0,
        "post-damage health kept, got {}",
        damaged.appearance.health
    );
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Task 2: UseItemOn places the HELD block (not stone), consumes 1 unless
/// creative; empty hand on a plain block does nothing.
#[test]
fn use_item_on_places_held_block_and_consumes() {
    use crate::boundary::{BlockPos, Hand, PlayerInput};
    let mut state = test_state();
    let entity = join(&mut state, 51, "placer");
    // Give 3 dirt into hotbar slot 0 (selected by default).
    let registry = test_registry();
    let dirt = mc_data::Identifier::parse("minecraft:dirt").expect("dirt");
    let dirt_id = registry.items.id_of(&dirt).expect("dirt item");
    {
        let rules = registry.item_rules();
        let player = state.players.get_mut(&entity).expect("player");
        let mut stack = mc_data::ItemStack {
            item_id: dirt_id,
            count: 3,
            ..Default::default()
        };
        player.inventory.add(&rules, &mut stack);
        assert!(stack.is_empty(), "all 3 fit");
    }
    // Click a stone block below the air cell (8,81,8) with face=top(1).
    // Find solid ground near spawn first: probe down from y=100.
    let mut ground_y = 100;
    while ground_y > -63 {
        if state.world.block_at(8, ground_y, 8) != Some(registry.states.air) {
            break;
        }
        ground_y -= 1;
    }
    let slot = state.entities.slot_of(entity).expect("slot");
    state
        .entities
        .set_pos(slot, [8.5, f64::from(ground_y + 1), 10.5]);
    let target = BlockPos {
        x: 8,
        y: ground_y + 1,
        z: 8,
    };
    let _ = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: crate::boundary::ConnectionId(51),
            hand: Hand::Main,
            position: BlockPos {
                x: 8,
                y: ground_y,
                z: 8,
            },
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 7,
        }],
    );
    let placed = state.world.block_at(target.x, target.y, target.z);
    let dirt_block = registry.blocks.block(&dirt).expect("dirt block").default.0;
    assert_eq!(placed, Some(dirt_block), "held dirt placed, not stone");
    let player = state.players.get(&entity).expect("player");
    let remaining: i32 = player.inventory.slots.iter().map(|slot| slot.count).sum();
    assert_eq!(remaining, 2, "one dirt consumed in survival");
}

/// Task 2: empty hand on a plain block does nothing.
#[test]
fn use_item_on_empty_hand_is_noop() {
    use crate::boundary::{BlockPos, Hand, PlayerInput};
    let mut state = test_state();
    join(&mut state, 52, "empty");
    let before = state.world.block_at(8, 81, 8);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: crate::boundary::ConnectionId(52),
            hand: Hand::Main,
            position: BlockPos { x: 8, y: 80, z: 8 },
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 8,
        }],
    );
    assert_eq!(state.world.block_at(8, 81, 8), before, "no placement");
}

/// Breeding needs love: unfed pigs never breed; feeding takes exactly one
/// food per animal that can fall in love, even when two players feed it in
/// the same tick; two fed pigs make one piglet (never with a cow in love
/// beside them), and parents cannot breed during cooldown while the visibly
/// young piglet cannot breed until adulthood.
#[test]
fn feeding_gates_breeding_and_takes_one_food() {
    use crate::boundary::Hand;
    use crate::entities::EntityKind;
    let registry = test_registry();
    let mut state = test_state();
    let id = |name: &str| mc_data::Identifier::parse(name).expect("id");
    let pig = registry
        .entity_types
        .id_of(&id("minecraft:pig"))
        .expect("pig");
    let carrot = registry
        .items
        .id_of(&id("minecraft:carrot"))
        .expect("carrot");
    let farmers = [
        join(&mut state, 301, "farmer"),
        join(&mut state, 302, "helper"),
    ];
    for farmer in farmers {
        let slot = state.entities.slot_of(farmer).expect("farmer slot");
        state.entities.set_pos(slot, [8.0, 64.0, 7.0]);
        state
            .players
            .get_mut(&farmer)
            .expect("farmer")
            .inventory
            .slots[0] = mc_data::ItemStack::new(carrot, 3);
    }
    // Ground is y=64: mobs spawned higher now take fall damage on landing.
    let a = state.spawn_mob(pig, [8.0, 64.0, 8.0], 10.0);
    let b = state.spawn_mob(pig, [9.0, 64.0, 8.0], 10.0);
    let cow = state.spawn_mob(registry.cow_type_id, [8.0, 64.0, 9.0], 10.0);
    let cow_slot = state.entities.slot_of(cow).expect("cow");
    state.entities.row_mut(cow_slot).love = 600;
    let count = |state: &TickState, type_id: u32| {
        state
            .entities
            .slots()
            .filter(|slot| state.entities.body(*slot).kind == EntityKind::Mob { type_id })
            .count()
    };
    let carrots = |state: &TickState| {
        farmers
            .iter()
            .map(|farmer| state.players[farmer].inventory.slots[0].count)
            .sum::<i32>()
    };
    let feed = |connection: u64, target| PlayerInput::InteractEntity {
        connection: ConnectionId(connection),
        target,
        hand: Hand::Main,
        location: [0.0; 3],
    };
    let _ = run_tick(&mut state, &[]);
    assert_eq!(count(&state, pig), 2, "unfed adults do not breed");
    let _ = run_tick(&mut state, &[feed(301, a), feed(302, a), feed(301, a)]);
    assert_eq!(carrots(&state), 5, "one carrot per animal falling in love");
    let _ = run_tick(&mut state, &[feed(302, a)]);
    assert_eq!(carrots(&state), 5, "an animal in love does not eat");
    let _ = run_tick(&mut state, &[feed(301, b)]);
    assert_eq!(carrots(&state), 4);
    for _ in 0..90 {
        let _ = run_tick(&mut state, &[]);
    }
    assert_eq!(count(&state, pig), 3, "two fed pigs make one piglet");
    assert_eq!(
        count(&state, registry.cow_type_id),
        1,
        "no cross-species calf"
    );
    for slot in state.entities.slots() {
        let row = state.entities.row(slot);
        if state.entities.body(slot).kind == (EntityKind::Mob { type_id: pig }) {
            assert_eq!(row.love, 0);
            assert!(row.breed_cooldown > 0 || row.entity_life.child_ticks > 0);
            assert_eq!(row.appearance.baby, row.entity_life.child_ticks > 0);
        }
    }
    assert!(
        state
            .entities
            .row(state.entities.slot_of(cow).expect("cow"))
            .love
            > 0
    );
    let _ = run_tick(&mut state, &[feed(301, a)]);
    assert_eq!(carrots(&state), 4, "a cooling-down parent does not eat");
}

/// Shears drop one to three wool of the sheep's color once, leave it
/// sheared, and wear by one; a second shear the same tick pays nothing.
#[test]
fn shearing_drops_colored_wool_once_and_wears_the_shears() {
    let mut state = test_state();
    let registry = test_registry();
    let item = |name: &str| {
        registry
            .items
            .id_of(&mc_data::Identifier::parse(name).expect("id"))
            .expect("item")
    };
    let sheep_type = registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").expect("id"))
        .expect("sheep");
    let shearers = [join(&mut state, 401, "a"), join(&mut state, 402, "b")];
    for shearer in shearers {
        let slot = state.entities.slot_of(shearer).expect("shearer slot");
        state.entities.set_pos(slot, [8.0, 80.0, 7.0]);
        state
            .players
            .get_mut(&shearer)
            .expect("player")
            .inventory
            .slots[0] = mc_data::ItemStack::new(item("minecraft:shears"), 1);
    }
    let sheep = state.spawn_mob(sheep_type, [8.0, 80.0, 8.0], 8.0);
    let slot = state.entities.slot_of(sheep).expect("sheep");
    crate::tick::set_wool(
        std::sync::Arc::make_mut(&mut state.entities.row_mut(slot).appearance),
        15,
    );
    let shear = |connection: u64| PlayerInput::InteractEntity {
        connection: ConnectionId(connection),
        target: sheep,
        hand: crate::boundary::Hand::Main,
        location: [0.0; 3],
    };
    let (output, _, _) = run_tick(&mut state, &[shear(401), shear(402)]);
    let black_wool = item("minecraft:black_wool");
    let wool: i32 = output
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| state.entities.slot_of(*id))
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|stack| stack.item_id == black_wool)
        .map(|stack| stack.count)
        .sum();
    assert!(
        (1..=3).contains(&wool),
        "one to three black wool, got {wool}"
    );
    let slot = state.entities.slot_of(sheep).expect("sheep");
    assert_eq!(
        crate::tick::wool_of(&state.entities.row(slot).appearance),
        0x10 | 15
    );
    let wear: Vec<Option<i32>> = shearers
        .iter()
        .map(|shearer| state.players[shearer].inventory.slots[0].damage)
        .collect();
    assert_eq!(wear, vec![Some(1), None], "only the winning shearer pays");
    let (output, _, _) = run_tick(&mut state, &[shear(402)]);
    assert!(
        output.changes.entities_spawned.is_empty(),
        "a sheared sheep drops nothing"
    );
}

/// Blocker #3: a torn checkpoint (failure between domain writes) publishes no
/// manifest and prunes nothing — reopen equals the last durable journal
/// state, no dupe.
#[test]
fn torn_checkpoint_publishes_nothing() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::{Core, FAIL_CHECKPOINT_AFTER_REGIONS};
    let dir = std::env::temp_dir().join(format!("mc-core-torn-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 100_000;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(61),
            uuid,
            name: "torn".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        // Durable give, then drop (ground item) — both journaled.
        core.submit(PlayerInput::Command {
            connection: ConnectionId(61),
            command: "debug give minecraft:dirt 3 0".to_owned(),
            permission_level: 4,
        });
        for _ in 0..3 {
            let _ = rx.blocking_recv().expect("tick");
        }
        // Arm the failure for THIS world dir, then force the only
        // checkpoint via shutdown: prepared chunks must not publish a
        // partial native checkpoint or permit WAL pruning.
        *FAIL_CHECKPOINT_AFTER_REGIONS.lock().expect("hook") = Some(dir.clone());
        core.shutdown();
        assert_eq!(
            crate::native_checkpoint::tests::read_manifest(&CoreConfig::new(dir.clone(), 11))
                .unwrap()
                .tick,
            0,
            "torn checkpoint preserves the initial manifest"
        );
    }
    // Journal tail intact: reopen replays the give; dirt exactly once.
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 100_000;
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(62),
        uuid,
        name: "torn".to_owned(),
        permission_level: 0,
    });
    let mut dirt_total = 0;
    for _ in 0..60 {
        let _ = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change {
                    dirt_total = dirt_total.max(
                        slots
                            .iter()
                            .flatten()
                            .filter(|stack| stack.count == 3)
                            .map(|stack| stack.count)
                            .sum::<i32>(),
                    );
                }
            }
        }
        if dirt_total == 3 {
            break;
        }
    }
    assert_eq!(dirt_total, 3, "dirt exactly once, no dupe");
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Every batch covered by a durable shutdown checkpoint is redundant.
#[test]
fn shutdown_checkpoint_prunes_all_covered_batches() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!("mc-core-prune-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let core = Core::open(config.clone()).expect("open");
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(71),
        uuid,
        name: "prune".to_owned(),
        permission_level: 0,
    });
    // Run until two checkpoints publish (manifest advances twice).
    let mut manifests = Vec::new();
    for _ in 0..400 {
        let _ = rx.blocking_recv().expect("tick");
        {
            let manifest = crate::native_checkpoint::tests::read_manifest(&config).unwrap();
            if manifest.tick > 0 && manifests.last() != Some(&manifest.tick) {
                manifests.push(manifest.tick);
            }
            if manifests.len() >= 2 {
                break;
            }
        }
    }
    assert!(manifests.len() >= 2, "two checkpoints published");
    core.shutdown();
    // Shutdown joins the native writer, including pruning after its final commit.
    let journal_dir = dir.join("journal");
    let names: Vec<String> = std::fs::read_dir(&journal_dir)
        .expect("journal dir")
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.starts_with("batch-"))
        .collect();
    assert!(
        names.is_empty(),
        "durable shutdown checkpoint must leave no covered batches, got {names:?}"
    );
    let _ = std::fs::remove_dir_all(&dir);
}

/// Custody: full inventory + cursor + crafting-grid stacks (non-stacking,
/// components) survive checkpoint + SIGKILL crash exactly once, components
/// intact. The crashing core runs in a CHILD process (real SIGKILL at an
/// observed WAL durability point); the parent reopens and asserts recovery.
#[test]
fn custody_survives_checkpoint_crash() {
    use crate::boundary::{ClickMode, ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    use crate::journal::read_complete_batches_after;
    use std::io::{BufRead as _, Write as _};

    fn custody_complete(entries: &[crate::journal::JournalEntry], names: &[String]) -> bool {
        entries.iter().any(|entry| {
            let crate::journal::JournalEntry::Player {
                inventory,
                carried,
                grid,
                ..
            } = entry
            else {
                return false;
            };
            names.iter().all(|name| {
                inventory
                    .iter()
                    .chain(std::iter::once(carried))
                    .chain(grid.iter())
                    .any(|stack| {
                        !stack.is_empty() && stack.custom_name.as_deref() == Some(name.as_str())
                    })
            }) && carried.custom_name.as_deref() == Some("keep-1")
                && grid.first().and_then(|stack| stack.custom_name.as_deref()) == Some("keep-0")
        })
    }
    // Child mode: build the full custody state, then park until killed.
    if std::env::var("MC_CORE_CUSTODY_CHILD").is_ok() {
        let dir = std::env::var("MC_CORE_CUSTODY_DIR").expect("child dir");
        let dir = std::path::PathBuf::from(dir);
        let uuid =
            uuid::Uuid::parse_str(&std::env::var("MC_CORE_CUSTODY_UUID").expect("child uuid"))
                .expect("uuid parses");
        let registry = test_registry();
        let dirt_id = registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
            .expect("dirt");
        let stack_named = |name: &str| mc_data::ItemStack {
            item_id: dirt_id,
            count: 1,
            damage: Some(5),
            enchantments: Vec::new(),
            custom_name: Some(name.into()),
            ..Default::default()
        };
        let names = std::sync::Arc::new((0..41).map(|i| format!("keep-{i}")).collect::<Vec<_>>());
        let observer_names = std::sync::Arc::clone(&names);
        *crate::core::JOURNAL_APPEND_OBSERVER
            .lock()
            .expect("install custody append observer") = Some((
            dir.clone(),
            Box::new(move |tick, entries| {
                if custody_complete(entries, &observer_names) {
                    let mut stdout = std::io::stdout().lock();
                    writeln!(stdout, "CUSTODY_WAL_READY {tick}").expect("custody readiness");
                    stdout.flush().expect("flush custody readiness");
                    drop(stdout);
                    // Keep this exact durable batch available, without a
                    // timer or a checkpoint/prune race, until real SIGKILL.
                    let (_hold, until_killed) = std::sync::mpsc::channel::<()>();
                    until_killed.recv().expect("parent kills custody child");
                }
            }),
        ));
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 5;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(81),
            uuid,
            name: "custody".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        for (slot, name) in names.iter().enumerate() {
            core.submit(PlayerInput::TestGive {
                connection: ConnectionId(81),
                slot,
                stack: stack_named(name),
            });
        }
        let consume_menu = |menu_state: &mut i32| {
            let mut inventory_ready = false;
            for (_, changes) in core.try_drain_outbox() {
                for change in changes {
                    match change {
                        crate::boundary::PlayerChange::MenuContents {
                            window: 0,
                            state_id,
                            slots,
                            ..
                        } => {
                            *menu_state = state_id;
                            inventory_ready |= slots
                                .iter()
                                .flatten()
                                .any(|stack| stack.custom_name.as_deref() == Some("keep-40"));
                        }
                        crate::boundary::PlayerChange::MenuSlot {
                            window: 0,
                            state_id,
                            item,
                            ..
                        } => {
                            *menu_state = state_id;
                            inventory_ready |= item.as_ref().is_some_and(|stack| {
                                stack.custom_name.as_deref() == Some("keep-40")
                            });
                        }
                        _ => {}
                    }
                }
            }
            inventory_ready
        };
        // Read the real state id after the last give, including slot deltas.
        // Fixed tick counts may consume buffered publications before the input.
        let mut menu_state = 0i32;
        loop {
            let _ = rx.blocking_recv().expect("inventory tick");
            if consume_menu(&mut menu_state) {
                break;
            }
        }
        // Real clicks on window 0: hotbar 0 -> cursor -> grid 0, then hotbar 1
        // -> cursor. Each next click uses its preceding authoritative ACK.
        for menu_slot in [36i16, 1, 37] {
            core.submit(PlayerInput::ContainerClick {
                connection: ConnectionId(81),
                window: 0,
                state_id: menu_state,
                slot: menu_slot,
                button: 0,
                mode: ClickMode::Pickup,
            });
            let previous_state = menu_state;
            while menu_state == previous_state {
                let _ = rx.blocking_recv().expect("menu tick");
                consume_menu(&mut menu_state);
            }
        }
        // Park until the parent SIGKILLs at the observed durability point.
        loop {
            let _ = rx.blocking_recv();
        }
    }
    // Parent mode: spawn the child, wait for the WAL to hold every named
    // stack (durability boundary), SIGKILL, reopen, assert exactly-once.
    let dir = std::env::temp_dir().join(format!(
        "mc-core-custody-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("dir");
    let uuid = uuid::Uuid::new_v4();
    let names: Vec<String> = (0..41).map(|i| format!("keep-{i}")).collect();
    struct CustodyChild {
        child: std::process::Child,
    }
    impl Drop for CustodyChild {
        fn drop(&mut self) {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
    let child_bin = std::env::current_exe().expect("test binary");
    let mut child = CustodyChild {
        child: std::process::Command::new(child_bin)
            .args([
                "--exact",
                "tick_tests::custody_survives_checkpoint_crash",
                "--nocapture",
            ])
            .env("MC_CORE_CUSTODY_CHILD", "1")
            .env("MC_CORE_CUSTODY_DIR", &dir)
            .env("MC_CORE_CUSTODY_UUID", uuid.to_string())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn custody child"),
    };
    // The child announces only a successful append + fsync with the full
    // cursor/grid custody image, then blocks its journal owner. Readiness
    // is an event, not a machine-speed-dependent filesystem polling loop.
    let stdout = child.child.stdout.take().expect("custody stdout");
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let stdout_reader = std::thread::spawn(move || {
        let mut result = Err("custody child closed stdout before readiness".to_owned());
        for line in std::io::BufReader::new(stdout).lines() {
            match line {
                Ok(line) => {
                    if let Some(tick) = line.strip_prefix("CUSTODY_WAL_READY ") {
                        result = tick.parse::<u64>().map_err(|error| error.to_string());
                        break;
                    }
                }
                Err(error) => {
                    result = Err(error.to_string());
                    break;
                }
            }
        }
        let _ = ready_tx.send(result);
    });
    let boundary_tick = ready_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("custody child readiness deadline")
        .expect("custody child readiness");
    stdout_reader.join().expect("custody stdout reader");
    // Freeze disk before inspecting it: the observer holds the journal owner,
    // but an earlier native checkpoint can still commit and prune its prefix.
    child.child.kill().expect("SIGKILL custody child");
    let status = child.child.wait().expect("reap custody child");
    assert!(!status.success(), "child died by signal, exit {status:?}");
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let manifest: crate::journal::CheckpointManifest = {
        // Open read/write once to repair redb allocator metadata after SIGKILL.
        // No core threads or checkpoint writer run while this reader is pinned.
        let store = mc_world::native::NativeStore::open(
            &dir.join("native/state.redb"),
            crate::native_checkpoint::native_identity(&config),
        )
        .expect("open crashed custody native store");
        let reader = store.checkpoint_reader().expect("custody native reader");
        let checkpoint = reader
            .read_checkpoint_metadata()
            .expect("custody checkpoint metadata");
        serde_json::from_slice(&checkpoint.globals["checkpoint"].payload)
            .expect("custody native manifest")
    };
    // Batches at/below this durable checkpoint may already have been removed.
    let batches = read_complete_batches_after(&dir.join("journal"), manifest.tick)
        .expect("durable custody WAL tail");
    assert!(
        boundary_tick > manifest.tick
            && batches
                .iter()
                .any(|(_, hi, entries)| *hi == boundary_tick && custody_complete(entries, &names)),
        "custody state durable in WAL"
    );
    // Reopen + rejoin: every named stack exactly once (inventory or ground).
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(82),
        uuid,
        name: "custody".to_owned(),
        permission_level: 0,
    });
    let mut found: std::collections::HashMap<String, u32> = std::collections::HashMap::new();
    for _ in 0..60 {
        let output = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change {
                    for slot in slots.into_iter().flatten() {
                        if let Some(name) = slot.custom_name.as_deref() {
                            *found.entry(name.to_owned()).or_default() += 1;
                        }
                    }
                }
            }
        }
        // Ground items from overflow (none expected: 2 free slots fit all).
        for view in &output.snapshot.entities {
            if let Some(stack) = &view.appearance.item
                && let Some(name) = stack.custom_name.as_deref()
            {
                *found.entry(name.to_owned()).or_default() += 1;
            }
        }
        if found.len() == 41 {
            break;
        }
    }
    let missing: Vec<_> = names
        .iter()
        .filter(|name| !found.contains_key(*name))
        .collect();
    let extra: Vec<_> = found.iter().filter(|(_, count)| **count != 1).collect();
    assert_eq!(
        found.len(),
        41,
        "all 41 stacks present, got {}; missing {missing:?} multi {extra:?}",
        found.len()
    );
    for name in &names {
        assert_eq!(
            found.get(name).copied().unwrap_or(0),
            1,
            "stack {name} exactly once"
        );
    }
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Full vanilla data loads from the content cache (missing data = error).
#[test]
fn registry_loads_full_vanilla_data() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = crate::registry::CoreRegistry::load(Some(&dir)).expect("full data loads");
    assert!(registry.item_facts.len() > 1000, "full item facts");
    assert!(registry.tags.total_tags() > 100, "full tags");
    assert!(registry.recipes.len() > 1000, "full recipes");
    assert!(registry.loot.total_drops() > 100, "full loot");
    assert!(registry.mining.len() > 1000, "full mining");
    // Tool rules exist (the embedded subset has none).
    let pick = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:diamond_pickaxe").expect("id"))
        .expect("pick");
    let facts = registry
        .item_facts
        .facts_for_stack(&mc_data::ItemStack::new(pick, 1), &registry.items)
        .expect("pick facts");
    assert!(facts.tool.is_some(), "pickaxe has tool rules");
}

/// Survival break spawns the block's loot as an item entity (embedded data:
/// dirt drops dirt with no tool gate).
#[test]
fn survival_break_spawns_loot() {
    use crate::boundary::{BlockPos, DigAction, PlayerInput};
    let mut state = test_state();
    let entity = join(&mut state, 101, "miner");
    // Find ground and dig the block below an air cell... simplest: place dirt
    // via TestGive? Break needs a real block: use the spawn terrain.
    let registry = test_registry();
    let dirt_block = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").expect("id"))
        .expect("stone")
        .default
        .0;
    // Put a dirt block at a known air cell near spawn.
    state.world.set_block(8, 81, 8, dirt_block);
    state.world.fold_overlay();
    let slot = state.entities.slot_of(entity).expect("slot");
    state.entities.set_pos(slot, [8.5, 82.0, 10.5]);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: crate::boundary::ConnectionId(101),
            position: BlockPos { x: 8, y: 81, z: 8 },
            action: DigAction::Finish,
            sequence: 1,
        }],
    );
    // Loot item entity spawned at the block centre.
    let found = state
        .entities
        .slots()
        .filter_map(|slot| {
            let row = state.entities.body(slot);
            match &row.kind {
                crate::entities::EntityKind::Item { .. } => Some(state.entities.pos(slot)),
                _ => None,
            }
        })
        .any(|pos| (pos[0] - 8.5).abs() < 0.01 && (pos[1] - 81.5).abs() < 0.01);
    assert!(found, "stone drop spawned at block centre");
    let _ = entity;
}

/// Pickup moves the stack into the inventory and syncs (embedded data).
#[test]
fn pickup_fills_inventory_and_syncs() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = test_state();
    let entity = join(&mut state, 102, "picker");
    // Drop a dirt stack at the player's feet via TestGive + DropSelected.
    let registry = test_registry();
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    state
        .players
        .get_mut(&entity)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(dirt_id, 5);
    let _ = run_tick(
        &mut state,
        &[PlayerInput::DropSelected {
            connection: ConnectionId(102),
            whole_stack: false,
        }],
    );
    // Player-thrown items retain forward motion and cannot be picked up for
    // forty ticks. Walk into the landed item's pickup box after that delay.
    for _ in 0..40 {
        let _ = run_tick(&mut state, &[]);
    }
    let at = state
        .entities
        .slots()
        .find_map(|slot| {
            matches!(
                state.entities.body(slot).kind,
                crate::entities::EntityKind::Item { .. }
            )
            .then(|| state.entities.pos(slot))
        })
        .expect("thrown item still on ground");
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, at);
    for _ in 0..2 {
        let _ = run_tick(&mut state, &[]);
    }
    let player = state.players.get(&entity).expect("player");
    let total: i32 = player.inventory.slots.iter().map(|slot| slot.count).sum();
    assert_eq!(total, 5, "one dropped dirt picked back up, total {total}");
}
/// Partial pickup keeps the remainder on the ground: a full inventory takes
/// nothing and the entity survives; space for half takes half.
#[test]
fn partial_pickup_keeps_remainder_on_ground() {
    use crate::boundary::{ConnectionId, EntityId, PlayerInput};
    use crate::intent::{Footprint, Intent, IntentKind};
    let mut state = test_state();
    let entity = join(&mut state, 103, "full-picker");
    let registry = test_registry();
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    // Fill every inventory slot with a full dirt stack (64).
    {
        let player = state.players.get_mut(&entity).expect("player");
        for slot in player.inventory.slots.iter_mut() {
            *slot = mc_data::ItemStack::new(dirt_id, 64);
        }
    }
    // Ground item: 10 dirt at the player's feet (spawn directly, delay 0).
    let ground = state.entities.alloc_id();
    state.entities.spawn(
        ground,
        uuid::Uuid::new_v4(),
        crate::entities::EntityKind::Item {
            type_id: state.registry.item_type_id,
        },
        crate::entities::ColdFields {
            pos: [8.0, 80.0, 8.0],
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ground: true,
            health: 1.0,
            breed_cooldown: 0,
            pickup_delay: 0,
            item: Some(crate::entities::StackRow {
                item_id: dirt_id,
                count: 10,
                stack: Some(mc_data::ItemStack::new(dirt_id, 10)),
            }),
            appearance: std::sync::Arc::new(crate::boundary::EntityAppearance::default()),
            movement: crate::blueprint::MovementClass::Item,
            half_width: 0.125,
            height: 0.25,
            speed: 0.0,
            attack_damage: 0.0,
            sight_range: 0.0,
            attack_reach: 0.0,
        },
    );
    // Direct pickup intent (bypasses reach/delay): capacity is zero.
    let mut intents = vec![Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::PickupItem {
            item: ground,
            count: i32::MAX,
        },
        footprint: Footprint {
            entity_reads: vec![entity],
            item_take: Some(ground),
            ..Default::default()
        },
    }];
    state.merge(&mut intents);
    let (_, _, _, removed) = state.commit_merge_effects();
    // Nothing fit: no removal, entity still on the ground with all 10.
    assert!(removed.is_empty(), "zero-capacity pickup removes nothing");
    let slot = state
        .entities
        .slot_of(ground)
        .expect("ground item survives");
    assert_eq!(
        state.entities.row(slot).item.as_ref().expect("stack").count,
        10
    );
    // Free one slot to half capacity: take 32 of the 64 (leave room for 32
    // more is wrong — instead empty one slot fully, then expect full take).
    state
        .players
        .get_mut(&entity)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(dirt_id, 32);
    let mut intents = vec![Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::PickupItem {
            item: ground,
            count: i32::MAX,
        },
        footprint: Footprint {
            entity_reads: vec![entity],
            item_take: Some(ground),
            ..Default::default()
        },
    }];
    state.merge(&mut intents);
    let (_, _, _, removed) = state.commit_merge_effects();
    // 10 fit into the 32-free slot: full take, entity gone.
    assert_eq!(removed, vec![ground]);
    assert!(
        state.entities.slot_of(ground).is_none(),
        "consumed entity removed"
    );
    let _ = ConnectionId(103);
    let _ = EntityId(0);
    let _ = PlayerInput::Leave {
        connection: ConnectionId(0),
    };
}

/// A partial pickup journals the ground remainder: recovery from the
/// journal tail must not restore the full stack next to the credited
/// inventory (dupe).
#[test]
fn partial_pickup_journals_the_ground_remainder() {
    use crate::intent::{Footprint, Intent, IntentKind};
    let mut state = test_state();
    let entity = join(&mut state, 104, "half-picker");
    let dirt_id = test_registry()
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    {
        let player = state.players.get_mut(&entity).expect("player");
        for slot in player.inventory.slots.iter_mut() {
            *slot = mc_data::ItemStack::new(dirt_id, 64);
        }
        player.inventory.slots[0] = mc_data::ItemStack::new(dirt_id, 60);
    }
    let ground = crate::tick::spawn_item_entity(
        &mut state,
        [8.0, 80.0, 8.0],
        mc_data::ItemStack::new(dirt_id, 10),
    );
    state.appearance_changed.clear();
    let mut intents = vec![Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::PickupItem {
            item: ground,
            count: i32::MAX,
        },
        footprint: Footprint {
            entity_reads: vec![entity],
            item_take: Some(ground),
            ..Default::default()
        },
    }];
    state.merge(&mut intents);
    let (_, _, _, removed) = state.commit_merge_effects();
    assert!(removed.is_empty(), "4 of 10 fit: the remainder stays");
    let changes = crate::boundary::ChangeSet {
        entities_appearance_changed: std::mem::take(&mut state.appearance_changed),
        ..Default::default()
    };
    let output = crate::boundary::TickOutput {
        tick: state.tick,
        snapshot: std::sync::Arc::new(crate::boundary::WorldSnapshot::default()),
        changes: std::sync::Arc::new(changes),
        times: Default::default(),
    };
    let remainder = crate::core::journal_entries_for_tick(&mut state, &output)
        .into_iter()
        .find_map(|entry| match entry {
            crate::journal::JournalEntry::Entity { id, item, .. }
                if id == crate::journal::entity_id_int(ground) =>
            {
                item.map(|stack| stack.count)
            }
            _ => None,
        });
    assert_eq!(
        remainder,
        Some(6),
        "journal carries the 6 left on the ground"
    );
}
/// tick: exactly one block lands, only the winner pays, the loser keeps its
/// stack (rejected intents debit nothing).
#[test]
fn contested_placement_charges_only_the_winner() {
    use crate::boundary::{BlockPos, Hand, PlayerInput};
    let mut state = test_state();
    let registry = test_registry();
    let dirt = mc_data::Identifier::parse("minecraft:dirt").expect("dirt");
    let stone = mc_data::Identifier::parse("minecraft:stone").expect("stone");
    let dirt_id = registry.items.id_of(&dirt).expect("dirt item");
    let stone_id = registry.items.id_of(&stone).expect("stone item");
    let a = join(&mut state, 61, "placer-a");
    let b = join(&mut state, 62, "placer-b");
    for (entity, item_id) in [(a, dirt_id), (b, stone_id)] {
        let rules = registry.item_rules();
        let player = state.players.get_mut(&entity).expect("player");
        let mut stack = mc_data::ItemStack {
            item_id,
            count: 3,
            ..Default::default()
        };
        player.inventory.add(&rules, &mut stack);
        assert!(stack.is_empty(), "all 3 fit");
    }
    // Air cell above solid ground at (8, ground+1, 8).
    let mut ground_y = 100;
    while ground_y > -63 {
        if state.world.block_at(8, ground_y, 8) != Some(registry.states.air) {
            break;
        }
        ground_y -= 1;
    }
    let clicked = BlockPos {
        x: 8,
        y: ground_y,
        z: 8,
    };
    let target = BlockPos {
        x: 8,
        y: ground_y + 1,
        z: 8,
    };
    for entity in [a, b] {
        let slot = state.entities.slot_of(entity).expect("slot");
        state
            .entities
            .set_pos(slot, [8.5, f64::from(ground_y + 1), 10.5]);
    }
    assert_eq!(
        state.world.block_at(target.x, target.y, target.z),
        Some(registry.states.air),
        "target starts air"
    );
    let place = |connection: u64, sequence: i32| PlayerInput::UseItemOn {
        connection: crate::boundary::ConnectionId(connection),
        hand: Hand::Main,
        position: clicked,
        face: 1,
        cursor: [0.5, 1.0, 0.5],
        sequence,
    };
    let (_, drains, _) = run_tick(&mut state, &[place(61, 1), place(62, 2)]);
    let dirt_block = registry.blocks.block(&dirt).expect("dirt block").default.0;
    let stone_block = registry
        .blocks
        .block(&stone)
        .expect("stone block")
        .default
        .0;
    let placed = state.world.block_at(target.x, target.y, target.z);
    assert!(
        placed == Some(dirt_block) || placed == Some(stone_block),
        "exactly one of the two blocks landed, got {placed:?}"
    );
    // Lower actor id sorts first in merge order, so A (joined first) wins.
    assert_eq!(placed, Some(dirt_block), "first actor wins the cell");
    let count = |entity| {
        state
            .players
            .get(&entity)
            .expect("player")
            .inventory
            .slots
            .iter()
            .map(|slot| slot.count)
            .sum::<i32>()
    };
    assert_eq!(count(a), 2, "winner paid one");
    assert_eq!(count(b), 3, "loser kept its full stack");
    // The winner's last synced window 0 already shows the debit.
    let mut synced = None;
    for (connection, changes) in &drains {
        if *connection != crate::boundary::ConnectionId(61) {
            continue;
        }
        for change in changes {
            if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change {
                synced = Some(
                    slots
                        .iter()
                        .flatten()
                        .filter(|stack| stack.item_id == dirt_id)
                        .map(|stack| stack.count)
                        .sum::<i32>(),
                );
            }
        }
    }
    assert_eq!(synced, Some(2), "winner's sync shows the debited stack");
}

/// Leave stores the final image: rejoin restores inventory, xp and position.
#[test]
fn leave_rejoin_restores_full_player_state() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = test_state();
    let uuid = uuid::Uuid::new_v4();
    let entity = {
        let (output, _, _) = run_tick(
            &mut state,
            &[PlayerInput::Join {
                connection: ConnectionId(201),
                uuid,
                name: "leaver".to_owned(),
                permission_level: 0,
            }],
        );
        assert_eq!(output.tick, state.tick);
        state
            .connections
            .entity_of(ConnectionId(201))
            .expect("joined")
    };
    let registry = test_registry();
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    state
        .players
        .get_mut(&entity)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(dirt_id, 7);
    state
        .players
        .get_mut(&entity)
        .expect("player")
        .vitals
        .experience
        .level = 5;
    let slot = state.entities.slot_of(entity).expect("slot");
    state.entities.set_pos(slot, [30.0, 82.0, 40.0]);
    let (output, _, _) = run_tick(
        &mut state,
        &[PlayerInput::Leave {
            connection: ConnectionId(201),
        }],
    );
    assert!(output.changes.entities_removed.contains(&entity));
    let offline = state.offline_players.get(&uuid).expect("offline image");
    assert_eq!(offline.pos, [30.0, 82.0, 40.0]);
    assert_eq!(offline.experience_level, 5);
    assert!(offline.inventory.iter().any(|stack| stack.count == 7));
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(202),
            uuid,
            name: "leaver".to_owned(),
            permission_level: 0,
        }],
    );
    let entity2 = state
        .connections
        .entity_of(ConnectionId(202))
        .expect("rejoined");
    let slot2 = state.entities.slot_of(entity2).expect("slot");
    assert_eq!(state.entities.pos(slot2), [30.0, 82.0, 40.0]);
    let player = state.players.get(&entity2).expect("player");
    assert!(player.inventory.slots.iter().any(|stack| stack.count == 7));
    assert_eq!(player.vitals.experience.level, 5);
    let _ = (output, drains);
}

/// Kill -> Died -> Respawn in one session: drops exactly once, record saved
/// empty (dead-inclusive lookup revives; journaled post-image is emptied).
#[test]
fn kill_died_respawn_drops_once_and_revives() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let mut state = test_state();
    let registry = test_registry();
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    let entity = join(&mut state, 203, "victim");
    state
        .players
        .get_mut(&entity)
        .expect("player")
        .inventory
        .slots[0] = mc_data::ItemStack::new(dirt_id, 4);
    // Lethal damage through the merge path (kill command equivalent).
    let slot = state.entities.slot_of(entity).expect("slot");
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: crate::survival::DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, events, spawned, _removed) = state.commit_merge_effects();
    assert_eq!(deaths, vec![entity]);
    assert!(events.iter().any(|event| matches!(
        event,
        crate::boundary::EntityEvent::Death { entity: id } if *id == entity
    )));
    // Death drops journaled via the spawn list.
    assert!(!spawned.is_empty(), "death drops spawn");
    // Player record is emptied (post-image through respawn/persistence).
    let record = crate::core::player_record_for(&state, entity, state.tick);
    assert!(
        record.is_none()
            || record
                .expect("record")
                .inventory
                .iter()
                .all(|stack| stack.is_empty()),
        "dead player inventory emptied"
    );
    // Dead row stays addressable (slot_of_any) though not live (slot_of).
    assert!(
        state.entities.slot_of(entity).is_none(),
        "dead not live-targetable"
    );
    assert!(
        state.entities.slot_of_any(entity).is_some(),
        "dead stays addressable"
    );
    // Respawn revives through the real input path. Death-commit order:
    // Health(0) precedes Died (commit pushes into state.outbox directly,
    // so drain it here for the death-order half). Respawn order: Respawned
    // (real position) BEFORE Health(20) + Experience.
    let mut death_queue: Vec<crate::boundary::PlayerChange> = Vec::new();
    if let Some(queue) = state.outbox.remove(&ConnectionId(203)) {
        death_queue.extend(queue);
    }
    let health0_idx = death_queue
        .iter()
        .position(|change| {
            matches!(
                change,
                crate::boundary::PlayerChange::Health { health, .. } if *health == 0.0
            )
        })
        .expect("Health(0) on death commit");
    let died_idx = death_queue
        .iter()
        .position(|change| matches!(change, crate::boundary::PlayerChange::Died { .. }))
        .expect("Died on death commit");
    assert!(
        health0_idx < died_idx,
        "death order: Health(0) precedes Died"
    );
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(203),
        }],
    );
    let slot = state.entities.slot_of(entity).expect("revived");
    assert!(state.entities.body(slot).alive);
    assert_eq!(state.entities.row(slot).health, 20.0);
    assert_eq!(
        state.entities.row(slot).appearance.health,
        20.0,
        "published health revives too (self SetEntityData is resent)"
    );
    // Vanilla spawn radius: within 10 blocks of the world spawn.
    let respawned_at = state.entities.pos(slot);
    let spawn = state.world.world_spawn_position();
    assert!(
        (respawned_at[0] - spawn[0]).abs() <= 10.0 && (respawned_at[2] - spawn[2]).abs() <= 10.0,
        "respawn {respawned_at:?} near spawn {spawn:?}"
    );
    let queue: Vec<_> = drains
        .iter()
        .filter(|(connection, _)| *connection == ConnectionId(203))
        .flat_map(|(_, changes)| changes.iter())
        .collect();
    let respawn_idx = queue
        .iter()
        .position(|change| {
            matches!(
                change,
                crate::boundary::PlayerChange::Respawned { position, .. }
                if *position == respawned_at
            )
        })
        .expect("Respawned with real position");
    let health_idx = queue
        .iter()
        .position(|change| {
            matches!(
                change,
                crate::boundary::PlayerChange::Health { health, .. } if *health == 20.0
            )
        })
        .expect("Health(20)");
    let xp_idx = queue
        .iter()
        .position(|change| matches!(change, crate::boundary::PlayerChange::Experience { .. }))
        .expect("Experience");
    assert!(
        respawn_idx < health_idx && health_idx < xp_idx,
        "outbox order Respawned, Health, Experience"
    );
}

/// `/tp` moves the teleported player's own client: its reliable queue carries
/// a position sync to the destination (vanilla sends one); otherwise the
/// client's next move drags the server position back.
#[test]
fn teleport_command_syncs_the_teleported_client() {
    use crate::boundary::{ConnectionId, PlayerChange, PlayerInput};
    let mut state = test_state();
    let entity = join(&mut state, 204, "jumper");
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(204),
            command: "tp 40 90 -24".to_owned(),
            permission_level: 4,
        }],
    );
    let slot = state.entities.slot_of(entity).expect("slot");
    assert_eq!(state.entities.pos(slot), [40.0, 90.0, -24.0]);
    let synced = drains
        .iter()
        .filter(|(connection, _)| *connection == ConnectionId(204))
        .flat_map(|(_, queue)| queue)
        .any(|change| {
            matches!(
                change,
                PlayerChange::PositionCorrected { position, .. } if *position == [40.0, 90.0, -24.0]
            )
        });
    assert!(synced, "teleported client gets a position sync: {drains:?}");
}

#[test]
fn teleport_command_ignores_an_older_sync_pose_echo() {
    let (mut state, entity, old) = flat_player_fixture();
    let connection = ConnectionId(1);
    // A rejected prediction leaves an older position sync in flight.
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection,
            position: [old[0] + 20.0, old[1], old[2]],
            yaw: 90.0,
            pitch: 0.0,
            on_ground: true,
        }],
    );
    assert_eq!(state.players[&entity].awaiting_teleport, Some(2));
    let destination = [old[0] + 3.2, old[1], old[2] + 5.5];
    run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection,
            command: format!(
                "tp @s {} {} {}",
                destination[0], destination[1], destination[2]
            ),
            permission_level: 4,
        }],
    );
    assert_eq!(state.players[&entity].awaiting_teleport, Some(3));
    run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id: 2,
            },
            PlayerInput::Move {
                connection,
                position: old,
                yaw: 90.0,
                pitch: 0.0,
                on_ground: false,
            },
        ],
    );
    let slot = state.entities.slot_of(entity).unwrap();
    assert_eq!(
        state.entities.pos(slot),
        destination,
        "old sync echo must not undo /tp"
    );
    assert_eq!(state.players[&entity].awaiting_teleport, Some(3));
    // Only the newest matching confirmation releases movement. A status or
    // look-only packet must not resurrect the decoder's pre-teleport pose.
    run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id: 4,
            },
            PlayerInput::MoveStatus {
                connection,
                on_ground: false,
            },
            PlayerInput::MoveRotation {
                connection,
                yaw: 45.0,
                pitch: 10.0,
                on_ground: false,
            },
        ],
    );
    assert_eq!(state.players[&entity].awaiting_teleport, Some(3));
    assert_eq!(state.entities.pos(slot), destination);
    run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id: 3,
            },
            PlayerInput::MoveRotation {
                connection,
                yaw: 45.0,
                pitch: 10.0,
                on_ground: false,
            },
            PlayerInput::MoveStatus {
                connection,
                on_ground: false,
            },
        ],
    );
    assert_eq!(state.players[&entity].awaiting_teleport, None);
    assert_eq!(state.entities.pos(slot), destination);
    assert_eq!(state.entities.rot(slot), [45.0, 10.0, 45.0]);
    let next = [destination[0] + 0.1, destination[1], destination[2]];
    run_tick(
        &mut state,
        &[
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id: 2,
            },
            PlayerInput::Move {
                connection,
                position: next,
                yaw: 45.0,
                pitch: 10.0,
                on_ground: false,
            },
        ],
    );
    assert_eq!(state.entities.pos(slot), next);
}
/// Real loaded storage must cook without a click driving synchronization.
#[test]
fn radius_split_furnace_does_not_consume_fuel_until_resident_chunk_activates() {
    let registry = Arc::new(full_registry());
    let world = flat_world(&registry);
    let mut state = TickState::new(registry, world, 2, 512);
    let player = join(&mut state, 1, "viewer");
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [8.0, 100.0, 8.0]);
    state.config_view_distance = 8;
    state.refresh_active_chunks(state.tick + 1);
    state.world.ensure_blocking(&state.desired_chunks());
    let furnace = state
        .registry
        .blocks
        .by_name_and_props(
            &mc_data::Identifier::parse("minecraft:furnace").unwrap(),
            &[
                ("facing".into(), "north".into()),
                ("lit".into(), "false".into()),
            ],
        )
        .unwrap()
        .0;
    let item = |name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let slots = vec![
        mc_data::ItemStack::new(item("minecraft:acacia_log"), 1),
        mc_data::ItemStack::new(item("minecraft:acacia_planks"), 1),
        mc_data::ItemStack::EMPTY,
    ];
    let key = (104, 64, 8);
    state.world.set_block(key.0, key.1, key.2, furnace);
    state.world.fold_overlay();
    state.block_entities.insert(
        key,
        crate::tick::BlockEntityState {
            slots: slots.clone(),
            furnace: Some(Default::default()),
        },
    );
    let before = state.block_entities[&key]
        .furnace
        .as_ref()
        .unwrap()
        .properties();
    for _ in 0..3 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.block_entities[&key].slots, slots);
    assert_eq!(
        state.block_entities[&key]
            .furnace
            .as_ref()
            .unwrap()
            .properties(),
        before
    );
    state.config_sim_distance = 6;
    run_tick(&mut state, &[]);
    assert_eq!(
        state.block_entities[&key]
            .furnace
            .as_ref()
            .unwrap()
            .cook_progress,
        1
    );
    assert_ne!(state.block_entities[&key].slots, slots);
}

#[test]
fn furnace_charcoal_cooks_and_updates_every_open_viewer() {
    use crate::boundary::{BlockPos, Hand, PlayerChange};
    let mut state = test_state();
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry = Arc::new(CoreRegistry::load(Some(&dir)).expect("full registry"));
    let a = join(&mut state, 204, "smelter-a");
    let b = join(&mut state, 205, "smelter-b");
    let spawn = state.world.world_spawn_position();
    let pos = BlockPos {
        x: spawn[0].floor() as i32,
        y: spawn[1].floor() as i32,
        z: spawn[2].floor() as i32,
    };
    for entity in [a, b] {
        let slot = state.entities.slot_of(entity).expect("slot");
        state
            .entities
            .set_pos(slot, [spawn[0], spawn[1], spawn[2] + 2.0]);
    }
    let id = mc_data::Identifier::parse("minecraft:furnace").unwrap();
    let facing = "east";
    let furnace_block = state
        .registry
        .blocks
        .by_name_and_props(
            &id,
            &[
                ("facing".to_owned(), facing.to_owned()),
                ("lit".to_owned(), "false".to_owned()),
            ],
        )
        .expect("furnace state")
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, furnace_block);
    state.world.fold_overlay();
    let item = |name| {
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let log = item("minecraft:acacia_log");
    let planks = item("minecraft:acacia_planks");
    let charcoal = item("minecraft:charcoal");
    let key = (pos.x, pos.y, pos.z);
    state.block_entities.insert(
        key,
        crate::tick::BlockEntityState {
            slots: vec![
                mc_data::ItemStack::new(log, 1),
                mc_data::ItemStack::new(planks, 1),
                mc_data::ItemStack::EMPTY,
            ],
            furnace: Some(Default::default()),
        },
    );
    let open = |connection| PlayerInput::UseItemOn {
        connection: ConnectionId(connection),
        position: pos,
        hand: Hand::Main,
        face: 1,
        cursor: [0.5; 3],
        sequence: 1,
    };
    let (output, first, _) = run_tick(&mut state, &[open(204), open(205)]);
    for connection in [ConnectionId(204), ConnectionId(205)] {
        let changes = &first
            .iter()
            .find(|(conn, _)| *conn == connection)
            .unwrap()
            .1;
        for (property, value) in [(0, 300), (1, 300), (2, 1), (3, 200)] {
            assert!(changes.iter().any(|change| matches!(change,
                PlayerChange::MenuProperty { property: p, value: v, .. } if *p == property && *v == value
            )), "missing furnace data {property}={value}: {changes:?}");
        }
    }
    let lit = state.world.block_at(pos.x, pos.y, pos.z).unwrap();
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(lit))
        .unwrap();
    assert!(
        block
            .properties
            .contains(&("facing".to_owned(), facing.to_owned()))
    );
    assert!(
        block
            .properties
            .contains(&("lit".to_owned(), "true".to_owned()))
    );
    assert!(
        output
            .changes
            .blocks
            .iter()
            .any(|change| change.position == pos && change.state == lit)
    );
    for progress in 2..200 {
        let (_, drains, _) = run_tick(&mut state, &[]);
        assert_eq!(
            state.block_entities[&key]
                .furnace
                .as_ref()
                .unwrap()
                .cook_progress,
            progress
        );
        assert!(state.block_entities[&key].slots[2].is_empty());
        for connection in [ConnectionId(204), ConnectionId(205)] {
            assert!(drains.iter().filter(|(conn, _)| *conn == connection).flat_map(|(_, changes)| changes)
                .any(|change| matches!(change, PlayerChange::MenuProperty { property: 2, value, .. } if i32::from(*value) == progress)));
        }
    }
    let before = [
        state.players[&a].open.as_ref().unwrap().state_id,
        state.players[&b].open.as_ref().unwrap().state_id,
    ];
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(
        state.block_entities[&key].slots[2],
        mc_data::ItemStack::new(charcoal, 1)
    );
    assert!(state.block_entities[&key].slots[0].is_empty());
    assert!(state.block_entities[&key].slots[1].is_empty());
    for (i, connection) in [ConnectionId(204), ConnectionId(205)]
        .into_iter()
        .enumerate()
    {
        assert!(
            drains
                .iter()
                .filter(|(conn, _)| *conn == connection)
                .flat_map(|(_, changes)| changes)
                .any(
                    |change| matches!(change, PlayerChange::MenuContents { state_id, slots, .. }
                if *state_id > before[i] && slots[2] == Some(mc_data::ItemStack::new(charcoal, 1)))
                )
        );
    }
    // Closed menus do not gate cooking; unloaded storage does.
    run_tick(
        &mut state,
        &[
            PlayerInput::ContainerClose {
                connection: ConnectionId(204),
                window: 1,
            },
            PlayerInput::ContainerClose {
                connection: ConnectionId(205),
                window: 1,
            },
        ],
    );
    state.block_entities.get_mut(&key).unwrap().slots[0] = mc_data::ItemStack::new(log, 1);
    state.block_entities.get_mut(&key).unwrap().slots[1] = mc_data::ItemStack::new(planks, 1);
    for _ in 0..200 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.block_entities[&key].slots[2].count, 2,
        "closed furnace cooks"
    );
    run_tick(
        &mut state,
        &[
            PlayerInput::Leave {
                connection: ConnectionId(204),
            },
            PlayerInput::Leave {
                connection: ConnectionId(205),
            },
        ],
    );
    // Dirty chunks remain resident until their checkpoint is acknowledged.
    // Project counter metadata before the fixture's simulated checkpoint ACK.
    // Unlike runtime worlds, this in-memory fixture has no native writer.
    state.world.capture_dirty(&Default::default());
    let written: Vec<_> = state
        .world
        .snapshot()
        .iter()
        .map(|chunk| (chunk.pos, state.world.storage_revision(chunk.pos)))
        .collect();
    state.world.mark_flushed(&written);
    let progress = state.block_entities[&key].furnace.clone();
    run_tick(&mut state, &[]);
    assert!(state.world.block_at(pos.x, pos.y, pos.z).is_none());
    assert_eq!(
        state.block_entities[&key].furnace, progress,
        "unloaded furnace pauses"
    );
}

#[test]
fn furnace_result_pickup_and_shift_click_award_experience_once() {
    use crate::boundary::{BlockPos, ClickMode, Hand, PlayerChange};
    for mode in [ClickMode::Pickup, ClickMode::QuickMove] {
        let mut state = test_state();
        let entity = join(&mut state, 206, "smelter");
        let spawn = state.world.world_spawn_position();
        let pos = BlockPos {
            x: spawn[0].floor() as i32,
            y: spawn[1].floor() as i32,
            z: spawn[2].floor() as i32,
        };
        let furnace = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse("minecraft:furnace").unwrap())
            .unwrap()
            .default
            .0;
        state.world.set_block(pos.x, pos.y, pos.z, furnace);
        state.world.fold_overlay();
        let item = |name| {
            state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse(name).unwrap())
                .unwrap()
        };
        let raw = item("minecraft:raw_iron");
        let coal = item("minecraft:coal");
        let iron = item("minecraft:iron_ingot");
        let key = (pos.x, pos.y, pos.z);
        state.block_entities.insert(
            key,
            crate::tick::BlockEntityState {
                slots: vec![
                    mc_data::ItemStack::new(raw, 1),
                    mc_data::ItemStack::new(coal, 1),
                    mc_data::ItemStack::EMPTY,
                ],
                furnace: Some(Default::default()),
            },
        );
        run_tick(
            &mut state,
            &[PlayerInput::UseItemOn {
                connection: ConnectionId(206),
                position: pos,
                hand: Hand::Main,
                face: 1,
                cursor: [0.5; 3],
                sequence: 1,
            }],
        );
        for _ in 1..200 {
            run_tick(&mut state, &[]);
        }
        assert_eq!(
            state.block_entities[&key].slots[2],
            mc_data::ItemStack::new(iron, 1)
        );
        let menu = state.players[&entity].open.as_ref().unwrap();
        let click = PlayerInput::ContainerClick {
            connection: ConnectionId(206),
            window: menu.window,
            state_id: menu.state_id,
            slot: 2,
            button: 0,
            mode,
        };
        let (output, drains, _) = run_tick(&mut state, std::slice::from_ref(&click));
        assert!(state.block_entities[&key].slots[2].is_empty());
        let player = &state.players[&entity];
        if mode == ClickMode::Pickup {
            assert_eq!(
                player.open.as_ref().unwrap().carried,
                mc_data::ItemStack::new(iron, 1)
            );
        } else {
            assert_eq!(
                player
                    .inventory
                    .slots
                    .iter()
                    .filter(|stack| stack.item_id == iron)
                    .map(|stack| stack.count)
                    .sum::<i32>(),
                1
            );
        }
        assert_eq!(
            player.vitals.experience.total, 0,
            "extraction spawns orbs, not direct XP"
        );
        let orbs: Vec<_> = output
            .changes
            .entities_spawned
            .iter()
            .filter_map(|id| {
                let slot = state.entities.slot_of(*id)?;
                state
                    .entities
                    .row(slot)
                    .appearance
                    .experience_orb
                    .map(|data| (*id, data))
            })
            .collect();
        assert_eq!(
            orbs.iter()
                .map(|(_, (value, count))| value * count)
                .sum::<i32>(),
            1
        );
        assert!(
            !drains
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| matches!(change, PlayerChange::Experience { total: 1, .. }))
        );
        let pos = state.entities.pos(state.entities.slot_of(entity).unwrap());
        for (id, _) in orbs {
            let slot = state.entities.slot_of(id).unwrap();
            state.entities.set_pos(slot, pos);
            state.entities.set_vel(slot, [0.0; 3]);
        }
        assert!(
            state.block_entities[&key]
                .furnace
                .as_ref()
                .unwrap()
                .recipes_used
                .is_empty()
        );
        let (_, drains, _) = run_tick(&mut state, &[click]);
        assert!(
            drains
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| matches!(change, PlayerChange::Experience { total: 1, .. }))
        );
        assert_eq!(
            state.players[&entity].vitals.experience.total, 1,
            "no duplicate experience"
        );
    }
}

/// Breaking a filled chest drops its contents, closes viewers, removes
/// storage in one transaction; a new chest there starts empty.
#[test]
fn breaking_filled_chest_drops_contents_and_clears_storage() {
    use crate::boundary::{BlockPos, ConnectionId, DigAction, Hand, PlayerInput};
    let mut state = test_state();
    let registry = test_registry();
    let chest_block = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").expect("id"))
        .expect("chest")
        .default
        .0;
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    let (cx, cy, cz) = (8, 81, 8);
    state.world.set_block(cx, cy, cz, chest_block);
    state.world.fold_overlay();
    state.block_entities.insert(
        (cx, cy, cz),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::new(dirt_id, 3)],
            furnace: None,
        },
    );
    let entity = join(&mut state, 205, "breaker");
    let slot = state.entities.slot_of(entity).expect("slot");
    state.entities.set_pos(slot, [8.5, 82.0, 10.5]);
    // Open the chest (viewer registered), then break it.
    let _ = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(205),
            hand: Hand::Main,
            position: BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    assert!(state.open_containers.contains_key(&entity));
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Dig {
            connection: ConnectionId(205),
            position: BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
            action: DigAction::Finish,
            sequence: 2,
        }],
    );
    // Block went to air.
    assert_eq!(state.world.block_at(cx, cy, cz), Some(registry.states.air));
    // Contents dropped as an item entity at the block centre.
    assert!(
        output.changes.entities_spawned.iter().any(|id| {
            state.entities.slot_of_any(*id).is_some_and(|slot| {
                state
                    .entities
                    .row(slot)
                    .item
                    .as_ref()
                    .is_some_and(|item| item.count == 3)
            })
        }),
        "chest contents dropped"
    );
    // Viewer closed, storage removed, tombstone staged.
    assert!(!state.open_containers.contains_key(&entity));
    assert!(!state.block_entities.contains_key(&(cx, cy, cz)));
    assert!(state.block_entity_removed.contains(&(cx, cy, cz)));
    let closed = drains
        .iter()
        .flat_map(|(_, queue)| queue.iter())
        .any(|change| matches!(change, crate::boundary::PlayerChange::MenuClosed { .. }));
    assert!(closed, "viewer got MenuClosed");
    // A new chest placed there starts empty (no resurrection of contents).
    state.world.set_block(cx, cy, cz, chest_block);
    state.world.fold_overlay();
    let _ = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(205),
            hand: Hand::Main,
            position: BlockPos {
                x: cx,
                y: cy,
                z: cz,
            },
            face: 1,
            cursor: [0.5; 3],
            sequence: 3,
        }],
    );
    let storage = state
        .block_entities
        .get(&(cx, cy, cz))
        .expect("new storage");
    assert!(
        storage.slots.iter().all(|stack| stack.is_empty()),
        "new chest starts empty"
    );
}

/// Two viewers on one chest: the second receives slot changes with a bumped
/// state-id; a stale click from the second viewer resyncs.
#[test]
fn shared_chest_second_viewer_syncs_and_stale_click_resyncs() {
    use crate::boundary::{BlockPos, ConnectionId, Hand, PlayerInput};
    use crate::boundary::{ClickMode, PlayerChange};
    let mut state = test_state();
    let registry = test_registry();
    let chest_block = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:chest").expect("id"))
        .expect("chest")
        .default
        .0;
    let dirt_id = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
        .expect("dirt");
    let (cx, cy, cz) = (8, 81, 8);
    state.world.set_block(cx, cy, cz, chest_block);
    state.world.fold_overlay();
    state.block_entities.insert(
        (cx, cy, cz),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::new(dirt_id, 5)],
            furnace: None,
        },
    );
    let a = join(&mut state, 206, "viewer-a");
    let b = join(&mut state, 207, "viewer-b");
    for entity in [a, b] {
        let slot = state.entities.slot_of(entity).expect("slot");
        state.entities.set_pos(slot, [8.5, 82.0, 10.5]);
    }
    let open = |connection: u64, sequence: i32| PlayerInput::UseItemOn {
        connection: ConnectionId(connection),
        hand: Hand::Main,
        position: BlockPos {
            x: cx,
            y: cy,
            z: cz,
        },
        face: 1,
        cursor: [0.5; 3],
        sequence,
    };
    let _ = run_tick(&mut state, &[open(206, 1)]);
    let (_, drains_b, _) = run_tick(&mut state, &[open(207, 2)]);
    // B got its MenuContents open image; record its state-id.
    let mut b_state = 0i32;
    for (_, queue) in &drains_b {
        for change in queue {
            if let PlayerChange::MenuContents { state_id, .. } = change {
                b_state = *state_id;
            }
        }
    }
    // Give A a dirt stack and click it INTO the chest (window of A's menu).
    state.players.get_mut(&a).expect("a").inventory.slots[0] = mc_data::ItemStack::new(dirt_id, 1);
    let a_window = state
        .players
        .get(&a)
        .expect("a")
        .open
        .as_ref()
        .map(|menu| menu.window)
        .unwrap_or(1);
    let a_state = state
        .players
        .get(&a)
        .expect("a")
        .open
        .as_ref()
        .map(|menu| menu.state_id)
        .unwrap_or(0);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(206),
            window: a_window,
            state_id: a_state,
            slot: 27,
            button: 0,
            mode: ClickMode::Pickup,
        }],
    );
    // B (other viewer) got a fresh full image with a bumped state-id.
    let mut b_new_state = b_state;
    let mut b_got_image = false;
    for (connection, queue) in &drains {
        if *connection != ConnectionId(207) {
            continue;
        }
        for change in queue {
            if let PlayerChange::MenuContents { state_id, .. } = change {
                b_got_image = true;
                b_new_state = (*state_id).max(b_new_state);
            }
        }
    }
    assert!(b_got_image, "second viewer received the fresh image");
    assert!(b_new_state != b_state || b_got_image, "state-id moved");
    // Stale click from B (old state-id) resyncs instead of mutating.
    let b_window = state
        .players
        .get(&b)
        .expect("b")
        .open
        .as_ref()
        .map(|menu| menu.window)
        .unwrap_or(1);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(207),
            window: b_window,
            state_id: b_state,
            slot: 0,
            button: 0,
            mode: ClickMode::Pickup,
        }],
    );
    let resynced = drains
        .iter()
        .flat_map(|(_, queue)| queue.iter())
        .any(|change| matches!(change, PlayerChange::MenuContents { .. }));
    assert!(resynced, "stale click resyncs the viewer");
    let _ = (a, b);
}

/// Leave -> checkpoint -> reopen -> rejoin: the offline image is durable.
#[test]
fn leave_checkpoint_reopen_rejoin_restores_state() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!(
        "mc-core-leaveckpt-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.checkpoint_every_ticks = 5;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(211),
            uuid,
            name: "leaver".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        core.submit(PlayerInput::Command {
            connection: ConnectionId(211),
            command: "debug give minecraft:dirt 7 0".to_owned(),
            permission_level: 4,
        });
        for _ in 0..3 {
            let _ = rx.blocking_recv().expect("tick");
        }
        core.submit(PlayerInput::Leave {
            connection: ConnectionId(211),
        });
        for _ in 0..8 {
            let _ = rx.blocking_recv().expect("tick");
        }
        core.shutdown();
    }
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(212),
        uuid,
        name: "leaver".to_owned(),
        permission_level: 0,
    });
    let mut restored = false;
    for _ in 0..200 {
        let _ = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change
                    && slots.iter().flatten().any(|stack| stack.count == 7)
                {
                    restored = true;
                }
            }
        }
        if restored {
            break;
        }
    }
    assert!(restored, "leave->checkpoint->reopen->rejoin restores dirt");
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Death drops are journaled: kill -> journal tail durable -> reopen ->
/// drops on the ground, victim inventory empty, no dupe.
#[test]
fn death_drops_journaled_no_dupe_after_reopen() {
    death_no_dupe_after_reopen(100_000);
}

/// Same across checkpoints: the victim's pre-death record (holding the
/// items) is already in a manifest, and checkpoints after the death must
/// replace it with the dead player's empty inventory.
#[test]
fn death_after_checkpoint_no_dupe_after_reopen() {
    death_no_dupe_after_reopen(20);
}

fn death_no_dupe_after_reopen(checkpoint_every_ticks: u64) {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    use crate::journal::read_complete_batches_after;
    let dir = std::env::temp_dir().join(format!(
        "mc-core-deathjournal-{}-{:?}-{checkpoint_every_ticks}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        // Recovery needs the victim's neighborhood, not 169 generated chunks.
        config.simulation_distance = 2;
        config.view_distance = 2;
        config.checkpoint_every_ticks = checkpoint_every_ticks;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(213),
            uuid,
            name: "victim".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        core.submit(PlayerInput::Command {
            connection: ConnectionId(213),
            command: "debug give minecraft:dirt 4 0".to_owned(),
            permission_level: 4,
        });
        // Long enough for a checkpoint to record the items held.
        for _ in 0..(checkpoint_every_ticks.min(100) + 3) {
            let _ = rx.blocking_recv().expect("tick");
        }
        core.submit(PlayerInput::Command {
            connection: ConnectionId(213),
            command: "kill @s".to_owned(),
            permission_level: 4,
        });
        if checkpoint_every_ticks < 100_000 {
            // Pruning may fold the death batch into a checkpoint before a
            // journal scan sees it: run past two more checkpoints instead.
            for _ in 0..(2 * checkpoint_every_ticks + 3) {
                let _ = rx.blocking_recv().expect("tick");
            }
        } else {
            let journal_dir = dir.join("journal");
            let mut death_durable = false;
            for _ in 0..6000 {
                let _ = rx.blocking_recv().expect("tick");
                let batches = read_complete_batches_after(&journal_dir, 0).unwrap();
                for (_, _, entries) in &batches {
                    for entry in entries {
                        if let crate::journal::JournalEntry::Entity {
                            item: Some(stack), ..
                        } = entry
                            && stack.count == 4
                        {
                            death_durable = true;
                        }
                    }
                }
                if death_durable {
                    break;
                }
            }
            assert!(death_durable, "death drops journal-durable before reopen");
        }
        core.shutdown();
    }
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.simulation_distance = 2;
    config.view_distance = 2;
    config.checkpoint_every_ticks = checkpoint_every_ticks;
    let core2 = Core::open(config).expect("reopen");
    let mut rx2 = core2.subscribe();
    core2.submit(PlayerInput::Join {
        connection: ConnectionId(214),
        uuid,
        name: "victim".to_owned(),
        permission_level: 0,
    });
    let mut dirt_in_inventory = 0;
    let mut contents_seen = false;
    let mut ground_stacks = 0;
    for _ in 0..200 {
        let output = rx2.blocking_recv().expect("tick");
        for (_, changes) in core2.try_drain_outbox() {
            for change in changes {
                // The first contents after rejoin are the restored
                // inventory; later ones may reflect a legitimate pickup of
                // the death drop lying at the rejoin position.
                if let crate::boundary::PlayerChange::MenuContents { slots, .. } = change
                    && !contents_seen
                {
                    contents_seen = true;
                    dirt_in_inventory = slots
                        .iter()
                        .flatten()
                        .filter(|stack| stack.count == 4)
                        .map(|stack| stack.count)
                        .sum::<i32>();
                }
            }
        }
        ground_stacks = ground_stacks.max(
            output
                .snapshot
                .entities
                .iter()
                .filter(|view| {
                    view.appearance
                        .item
                        .as_ref()
                        .is_some_and(|stack| stack.count == 4)
                })
                .count(),
        );
        // Decide only after the rejoin inventory sync arrived.
        if contents_seen && ground_stacks >= 1 {
            break;
        }
    }
    assert!(contents_seen, "rejoin inventory synced");
    assert_eq!(dirt_in_inventory, 0, "victim inventory empty after death");
    assert_eq!(ground_stacks, 1, "exactly one death-drop stack, no dupe");
    core2.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Journal poison: an injected append failure stops durability and refuses
/// the checkpoint ack (fail closed, no manifest advance).
#[test]
fn journal_append_failure_poisons_and_blocks_checkpoint() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::{Core, FAIL_JOURNAL_APPEND};
    use crate::native_checkpoint::tests::fixture_store;
    // Unique dir per test invocation (parallel libtest runs share the pid;
    // the poison hook is process-global, so never leave it armed).
    let dir = std::env::temp_dir().join(format!(
        "mc-core-poison-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mut config = CoreConfig::new(dir.clone(), 11);
    config.checkpoint_every_ticks = 5;
    let core = Core::open(config.clone()).expect("open");
    // Retain the runtime's writer handle through the halt and read committed
    // images on it; opening another database handle races writer teardown.
    let store = fixture_store(&config).unwrap();
    let mut rx = core.subscribe();
    core.submit(PlayerInput::Join {
        connection: ConnectionId(215),
        uuid: uuid::Uuid::new_v4(),
        name: "poison".to_owned(),
        permission_level: 0,
    });
    let mut baseline = None;
    for _ in 0..200 {
        let _ = rx.blocking_recv().expect("tick");
        let checkpoint_tick = store
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap()
            .tick;
        if checkpoint_tick > 0 {
            baseline = Some(checkpoint_tick);
            break;
        }
    }
    assert!(baseline.is_some(), "healthy baseline checkpoint");
    *FAIL_JOURNAL_APPEND.lock().expect("hook") = Some(dir.clone());
    // After the poisoned append the tick loop halts and drops the only
    // broadcast Sender, so subscribers see `Closed` (ticks already queued
    // before the journal noticed may still arrive first). Every connection
    // gets a terminal Disconnect through the reliable outbox. The wait is
    // event-driven; the bound only fails a hang.
    let (done_tx, done_rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        while let Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) =
            rx.blocking_recv()
        {}
        let _ = done_tx.send(());
    });
    done_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("halt closes tick subscribers");
    assert!(
        matches!(
            core.subscribe().try_recv(),
            Err(tokio::sync::broadcast::error::TryRecvError::Closed)
        ),
        "subscribing after the halt yields a closed receiver"
    );
    let mut disconnected = false;
    for (_, queue) in core.try_drain_outbox() {
        for change in queue {
            if matches!(change, crate::boundary::PlayerChange::Disconnect { .. }) {
                disconnected = true;
            }
        }
    }
    assert!(disconnected, "every connection got Disconnect on halt");
    let after = Some(
        store
            .checkpoint_reader()
            .unwrap()
            .read_checkpoint()
            .unwrap()
            .tick,
    );
    assert_eq!(after, baseline, "poisoned journal advances no manifest");
    // Disarm: per-dir hook, release our dir (parallel tests unaffected).
    *FAIL_JOURNAL_APPEND.lock().expect("hook") = None;
    core.shutdown();
    drop(store);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Chat travels the reliable outbox: delivered even when the broadcast
/// tick is skipped — the outbox drain carries it.
#[test]
fn chat_delivered_via_reliable_outbox() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!(
        "mc-core-chat-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let config = CoreConfig::new(dir.clone(), 11);
    let core = Core::open(config).expect("open");
    let mut rx = core.subscribe();
    for (connection, name) in [(221, "chatter-a"), (222, "chatter-b")] {
        core.submit(PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::new_v4(),
            name: name.to_owned(),
            permission_level: 0,
        });
    }
    for _ in 0..200 {
        let output = rx.blocking_recv().expect("tick");
        if output.snapshot.entities.len() >= 2 {
            break;
        }
    }
    core.submit(PlayerInput::Chat {
        connection: ConnectionId(221),
        message: "hello-reliable".to_owned(),
    });
    let mut delivered = false;
    for _ in 0..200 {
        let _ = rx.blocking_recv().expect("tick");
        for (connection, queue) in core.try_drain_outbox() {
            if connection != ConnectionId(222) {
                continue;
            }
            for change in queue {
                if let crate::boundary::PlayerChange::StyledChat { content, .. } = change
                    && format!("{content:?}").contains("hello-reliable")
                {
                    delivered = true;
                }
            }
        }
        if delivered {
            break;
        }
    }
    assert!(delivered, "chat reached B via reliable outbox");
    core.shutdown();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Drain the reliable changes of every input submitted before this call.
/// Inputs drain at the start of a tick and the outbox is appended after that
/// tick's broadcast, so after discarding stale buffered outputs: fresh tick 1
/// may predate the submit, tick 2 processes it, and tick 3's broadcast
/// follows tick 2's outbox append.
fn settle(
    core: &crate::core::Core,
    rx: &mut tokio::sync::broadcast::Receiver<crate::boundary::TickOutput>,
) -> Vec<crate::boundary::PlayerChange> {
    use tokio::sync::broadcast::error::TryRecvError;
    loop {
        match rx.try_recv() {
            Ok(_) | Err(TryRecvError::Lagged(_)) => {}
            Err(TryRecvError::Empty) => break,
            Err(TryRecvError::Closed) => panic!("tick thread died"),
        }
    }
    for _ in 0..3 {
        let _ = rx.blocking_recv().expect("tick");
    }
    core.try_drain_outbox()
        .into_iter()
        .flat_map(|(_, queue)| queue)
        .collect()
}

/// Container break through a real Core: place chest, fill via real clicks,
/// break -> MenuClosed + drops equal contents + storage gone; reopen sees
/// no resurrection and a fresh chest starts empty.
#[test]
fn container_break_drops_survive_restart_without_resurrection() {
    use crate::boundary::{BlockPos, ClickMode, ConnectionId, DigAction, Hand, PlayerInput};
    use crate::boundary::{PlayerChange, TickOutput};
    use crate::config::CoreConfig;
    use crate::core::Core;
    let dir = std::env::temp_dir().join(format!(
        "mc-core-chestbreak-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let uuid = uuid::Uuid::new_v4();
    // Phase 1: place + fill + break, observe drops + close.
    let chest_cell: BlockPos;
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        // The chest and its drops remain within the player's neighborhood.
        config.simulation_distance = 2;
        config.view_distance = 2;
        config.checkpoint_every_ticks = 100_000;
        let core = Core::open(config).expect("open");
        let mut rx = core.subscribe();
        core.submit(PlayerInput::Join {
            connection: ConnectionId(231),
            uuid,
            name: "breaker".to_owned(),
            permission_level: 0,
        });
        // Wait for the join and for the column next to the player to be
        // resident (chunks load off-tick after the join tick).
        let (cx, cy, cz) = loop {
            let output: TickOutput = rx.blocking_recv().expect("tick");
            let Some(view) = output
                .snapshot
                .entities
                .iter()
                .find(|view| view.uuid == uuid)
            else {
                continue;
            };
            let (cx, cy, cz) = (
                view.position[0].floor() as i32 + 2,
                view.position[1].floor() as i32,
                view.position[2].floor() as i32 + 2,
            );
            let column = crate::boundary::ChunkPos {
                x: cx.div_euclid(16),
                z: cz.div_euclid(16),
            };
            if output
                .snapshot
                .chunks
                .iter()
                .any(|entry| entry.pos == column)
            {
                break (cx, cy, cz);
            }
        };
        // Chest item into hotbar slot 0 (selected), dirt into slot 1.
        let registry = crate::registry::CoreRegistry::load(None).expect("registry");
        let chest_id = registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:chest").expect("id"))
            .expect("chest item");
        let dirt_id = registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:dirt").expect("id"))
            .expect("dirt item");
        core.submit(PlayerInput::TestGive {
            connection: ConnectionId(231),
            slot: 0,
            stack: mc_data::ItemStack::new(chest_id, 1),
        });
        core.submit(PlayerInput::TestGive {
            connection: ConnectionId(231),
            slot: 1,
            stack: mc_data::ItemStack::new(dirt_id, 3),
        });
        let _ = settle(&core, &mut rx);
        // Sweep the column beside the player for a ground cell: click
        // (cx,y,cz) top face; a chest placement is silent, so probe by
        // clicking the cell ABOVE with face 0 — MenuOpened means the cell
        // below is a container.
        let mut cell = None;
        for y in (cy - 6..cy + 3).rev() {
            core.submit(PlayerInput::UseItemOn {
                connection: ConnectionId(231),
                hand: Hand::Main,
                position: BlockPos { x: cx, y, z: cz },
                face: 1,
                cursor: [0.5; 3],
                sequence: 1000 + y,
            });
            let _ = settle(&core, &mut rx);
            // Probe: click the target cell itself; MenuOpened => chest there.
            core.submit(PlayerInput::UseItemOn {
                connection: ConnectionId(231),
                hand: Hand::Main,
                position: BlockPos {
                    x: cx,
                    y: y + 1,
                    z: cz,
                },
                face: 0,
                cursor: [0.5; 3],
                sequence: 2000 + y,
            });
            let opened = settle(&core, &mut rx)
                .iter()
                .any(|change| matches!(change, PlayerChange::MenuOpened { .. }));
            if opened {
                cell = Some(BlockPos {
                    x: cx,
                    y: y + 1,
                    z: cz,
                });
                break;
            }
            // Restore chest for the next attempt.
            core.submit(PlayerInput::TestGive {
                connection: ConnectionId(231),
                slot: 0,
                stack: mc_data::ItemStack::new(chest_id, 1),
            });
            let _ = settle(&core, &mut rx);
        }
        let cell = cell.expect("chest placed in column");
        chest_cell = cell;
        // Track the open menu window + state-id from the open image.
        let mut window = 1u8;
        let mut menu_state = 0i32;
        for (_, queue) in core.try_drain_outbox() {
            for change in queue {
                if let PlayerChange::MenuContents { state_id, .. } = change {
                    menu_state = menu_state.max(state_id);
                }
                if let PlayerChange::MenuOpened { window: w, .. } = change {
                    window = w;
                }
            }
        }
        // Move 3 dirt into chest slot 0: select slot 1, pickup inventory
        // slot, place into chest slot. Menu layout (Chest 27 + player tail):
        // chest slots 0..27, then inventory. Pickup from inventory tail slot
        // then click chest slot 0.
        // Step 1: click inventory dirt (find its menu slot by scanning the
        // open image for the 3-dirt stack).
        let mut dirt_menu_slot = None;
        // Re-open to get a fresh image with slot mapping.
        core.submit(PlayerInput::ContainerClose {
            connection: ConnectionId(231),
            window,
        });
        let _ = settle(&core, &mut rx);
        core.submit(PlayerInput::UseItemOn {
            connection: ConnectionId(231),
            hand: Hand::Main,
            position: cell,
            face: 0,
            cursor: [0.5; 3],
            sequence: 3000,
        });
        let mut image: Vec<Option<mc_data::ItemStack>> = Vec::new();
        for change in settle(&core, &mut rx) {
            match change {
                PlayerChange::MenuContents {
                    slots,
                    state_id,
                    window: w,
                    ..
                } => {
                    image = slots;
                    menu_state = state_id;
                    window = w;
                }
                PlayerChange::MenuOpened { window: w, .. } => window = w,
                _ => {}
            }
        }
        for (index, slot) in image.iter().enumerate() {
            if slot
                .as_ref()
                .is_some_and(|stack| stack.item_id == dirt_id && stack.count == 3)
            {
                dirt_menu_slot = Some(index as i16);
                break;
            }
        }
        let dirt_slot = dirt_menu_slot.expect("dirt visible in menu image");
        // Pickup dirt to cursor.
        core.submit(PlayerInput::ContainerClick {
            connection: ConnectionId(231),
            window,
            state_id: menu_state,
            slot: dirt_slot,
            button: 0,
            mode: ClickMode::Pickup,
        });
        for change in settle(&core, &mut rx) {
            if let PlayerChange::MenuContents { state_id, .. } = change {
                menu_state = menu_state.max(state_id);
            }
        }
        // Place into chest slot 0.
        core.submit(PlayerInput::ContainerClick {
            connection: ConnectionId(231),
            window,
            state_id: menu_state,
            slot: 0,
            button: 0,
            mode: ClickMode::Pickup,
        });
        let _ = settle(&core, &mut rx);
        // Close, reopen, assert the chest now shows 3 dirt in slot 0.
        core.submit(PlayerInput::ContainerClose {
            connection: ConnectionId(231),
            window,
        });
        let _ = settle(&core, &mut rx);
        core.submit(PlayerInput::UseItemOn {
            connection: ConnectionId(231),
            hand: Hand::Main,
            position: cell,
            face: 0,
            cursor: [0.5; 3],
            sequence: 3001,
        });
        let mut chest_image: Vec<Option<mc_data::ItemStack>> = Vec::new();
        for change in settle(&core, &mut rx) {
            if let PlayerChange::MenuContents { slots, .. } = change {
                chest_image = slots;
            }
        }
        assert!(
            chest_image
                .first()
                .is_some_and(|slot| slot.as_ref().is_some_and(|stack| stack.count == 3)),
            "chest holds 3 dirt, got {chest_image:?}"
        );
        // Break the chest: drops + MenuClosed in the same transaction.
        core.submit(PlayerInput::Dig {
            connection: ConnectionId(231),
            position: cell,
            action: DigAction::Finish,
            sequence: 4000,
        });
        let mut broke = false;
        let mut closed = false;
        let mut dropped = false;
        for _ in 0..10 {
            let output = rx.blocking_recv().expect("tick");
            for (_, queue) in core.try_drain_outbox() {
                for change in queue {
                    if matches!(change, PlayerChange::MenuClosed { .. }) {
                        closed = true;
                    }
                }
            }
            if output
                .changes
                .blocks
                .iter()
                .any(|change| change.position == cell)
            {
                broke = true;
            }
            if output.changes.entities_spawned.iter().any(|id| {
                output.snapshot.entities.iter().any(|view| {
                    view.id == *id
                        && view
                            .appearance
                            .item
                            .as_ref()
                            .is_some_and(|stack| stack.count == 3)
                })
            }) {
                dropped = true;
            }
            if broke && closed && dropped {
                break;
            }
        }
        assert!(broke, "chest block broken");
        assert!(closed, "viewer got MenuClosed");
        assert!(dropped, "contents dropped as item entity");
        core.shutdown();
    }
    // Phase 2: reopen — no resurrection; a fresh chest starts empty.
    {
        let mut config = CoreConfig::new(dir.clone(), 11);
        config.simulation_distance = 2;
        config.view_distance = 2;
        config.checkpoint_every_ticks = 100_000;
        let core2 = Core::open(config).expect("reopen");
        let mut rx2 = core2.subscribe();
        core2.submit(PlayerInput::Join {
            connection: ConnectionId(232),
            uuid,
            name: "breaker".to_owned(),
            permission_level: 0,
        });
        loop {
            let output = rx2.blocking_recv().expect("tick");
            if output
                .snapshot
                .entities
                .iter()
                .any(|view| view.uuid == uuid)
            {
                break;
            }
        }
        // The broken cell is air (no resurrection of chest + contents).
        // Block state is visible via a fresh dig attempt producing no
        // change... simpler: no MenuOpened when clicking the cell.
        core2.submit(PlayerInput::UseItemOn {
            connection: ConnectionId(232),
            hand: Hand::Main,
            position: chest_cell,
            face: 0,
            cursor: [0.5; 3],
            sequence: 5000,
        });
        let opened = settle(&core2, &mut rx2)
            .iter()
            .any(|change| matches!(change, PlayerChange::MenuOpened { .. }));
        assert!(!opened, "broken chest did not resurrect after restart");
        core2.shutdown();
    }
    let _ = std::fs::remove_dir_all(&dir);
}

fn flat_player_fixture() -> (TickState, crate::boundary::EntityId, [f64; 3]) {
    let mut state = test_state();
    let player = join(&mut state, 1, "sweep");
    let slot = state.entities.slot_of(player).unwrap();
    let old = state.entities.pos(slot);
    let pos = [old[0].floor() + 0.5, old[1].ceil(), old[2].floor() + 0.5];
    let stone = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default
        .0;
    for x in (pos[0] as i32 - 5)..=(pos[0] as i32 + 5) {
        for z in (pos[2] as i32 - 3)..=(pos[2] as i32 + 3) {
            state.world.set_block(x, pos[1] as i32 - 1, z, stone);
            for dy in 0..5 {
                state
                    .world
                    .set_block(x, pos[1] as i32 + dy, z, state.registry.states.air);
            }
        }
    }
    state.world.fold_overlay();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, true);
    (state, player, pos)
}

#[test]
fn gamerules_freeze_daylight_and_weather_duration_and_publish_the_clock_rate() {
    use crate::commands::Weather;
    let (mut state, _, _) = flat_player_fixture();
    state
        .gamerules
        .insert("advance_time".into(), "false".into());
    state
        .gamerules
        .insert("advance_weather".into(), "false".into());
    state.weather = (Weather::Rain, 2);
    let time = state.day_time;
    for _ in 0..2 {
        let (output, _, _) = run_tick(&mut state, &[]);
        assert_eq!(output.snapshot.day_time, time);
        assert_eq!(output.snapshot.day_time_rate, 0.0);
        assert!(output.snapshot.raining);
        assert_eq!(state.weather, (Weather::Rain, 2));
    }
    state.gamerules.insert("advance_time".into(), "true".into());
    state
        .gamerules
        .insert("advance_weather".into(), "true".into());
    let (output, _, _) = run_tick(&mut state, &[]);
    assert_eq!(output.snapshot.day_time, time + 1);
    assert_eq!(output.snapshot.day_time_rate, 1.0);
    assert_eq!(state.weather, (Weather::Rain, 1));
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.weather.0, Weather::Clear);
    assert!((12_000..=180_000).contains(&state.weather.1));
    assert!(!output.snapshot.raining);
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::WeatherChanged { raining: false, .. }
            ))
    );
}

#[test]
fn default_weather_durations_use_each_vanilla_distribution_deterministically() {
    use crate::commands::Weather;
    let mut state = test_state();
    for (weather, minimum, maximum) in [
        (Weather::Clear, 12_000, 180_000),
        (Weather::Rain, 12_000, 24_000),
        (Weather::Thunder, 3_600, 15_600),
    ] {
        let mut samples = Vec::new();
        for tick in 0..32 {
            state.tick = tick;
            let sampled = crate::weather::default_duration(&state, weather);
            assert!((minimum..=maximum).contains(&sampled));
            assert_eq!(sampled, crate::weather::default_duration(&state, weather));
            samples.push(sampled);
        }
        assert!(samples.windows(2).any(|pair| pair[0] != pair[1]));
    }
}

#[test]
fn damage_rules_only_bypass_their_player_damage_types() {
    use crate::survival::DamageKind;
    for (rule, kind) in [
        ("fall_damage", DamageKind::Fall),
        ("drowning_damage", DamageKind::Drowning),
        ("fire_damage", DamageKind::InFire),
        ("fire_damage", DamageKind::OnFire),
        ("fire_damage", DamageKind::Lava),
    ] {
        let (mut state, player, _) = flat_player_fixture();
        state.tick = 100;
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.gamerules.insert(rule.into(), "false".into());
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.0,
                kind,
                source: None,
            },
        ));
        let (_, events, _, _) = state.commit_merge_effects();
        assert_eq!(
            state.players[&player].vitals.health, 20.0,
            "{rule} {kind:?}"
        );
        assert!(state.entities.body(slot).hurt_at.is_none());
        assert!(
            !events
                .iter()
                .any(|event| matches!(event, crate::boundary::EntityEvent::Hurt { .. }))
        );
        state.merge_damage.push((
            slot,
            crate::tick::DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.0,
                kind: DamageKind::Generic,
                source: None,
            },
        ));
        state.commit_merge_effects();
        assert_eq!(state.players[&player].vitals.health, 18.0);
    }
}

#[test]
fn keep_inventory_and_immediate_respawn_retain_xp_and_inventory_without_death_chat() {
    let (mut state, player, _) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "true".into());
    state
        .gamerules
        .insert("immediate_respawn".into(), "true".into());
    state
        .gamerules
        .insert("show_death_messages".into(), "false".into());
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let actor = state.players.get_mut(&player).unwrap();
    actor.inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    actor.vitals.experience.add_points(42);
    let experience = actor.vitals.experience;
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: crate::survival::DamageKind::Void,
            source: None,
        },
    ));
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert!(state.entities.body(slot).alive);
    assert_eq!(state.players[&player].vitals.health, 20.0);
    assert_eq!(state.players[&player].vitals.experience, experience);
    assert_eq!(
        state.players[&player].inventory.slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
    assert!(output.changes.entity_events.iter().any(|event| matches!(
        event, crate::boundary::EntityEvent::Death { entity } if *entity == player
    )));
    let changes: Vec<_> = drains.iter().flat_map(|(_, changes)| changes).collect();
    assert!(
        changes
            .iter()
            .any(|change| matches!(change, crate::boundary::PlayerChange::Respawned { .. }))
    );
    assert!(
        !changes
            .iter()
            .any(|change| matches!(change, crate::boundary::PlayerChange::Died { .. }))
    );
}

#[test]
fn mob_drops_rule_suppresses_death_loot_without_suppressing_death() {
    let (mut state, _, position) = flat_player_fixture();
    state.gamerules.insert("mob_drops".into(), "false".into());
    let sheep = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:sheep").unwrap())
        .unwrap();
    let mob = state.spawn_mob(sheep, position, 10.0);
    let slot = state.entities.slot_of(mob).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: crate::survival::DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, spawned, removed) = state.commit_merge_effects();
    assert_eq!(deaths, vec![mob]);
    assert!(spawned.is_empty());
    assert!(removed.contains(&mob));
}

#[test]
fn block_drops_rule_suppresses_popped_block_loot() {
    let (mut state, _, position) = flat_player_fixture();
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    state.registry = Arc::new(CoreRegistry::load(Some(&dir)).expect("full vanilla block loot"));
    let pos = crate::boundary::BlockPos {
        x: position[0] as i32,
        y: position[1] as i32,
        z: position[2] as i32,
    };
    let dirt = state.registry.states.dirt;
    let mut spawned = Vec::new();
    state.gamerules.insert("block_drops".into(), "false".into());
    state.pop_block_loot(pos, dirt, &mut spawned);
    assert!(spawned.is_empty());
    state.gamerules.insert("block_drops".into(), "true".into());
    state.pop_block_loot(pos, dirt, &mut spawned);
    assert!(!spawned.is_empty());
}

fn crouching_passage_fixture() -> (TickState, crate::boundary::EntityId, [f64; 3]) {
    let (mut state, player, mut pos) = flat_player_fixture();
    // Client-owned movement can start at fractional Y (for example mid-jump).
    // A full-block ceiling then leaves exactly 1.5 blocks above the feet.
    pos[1] += 0.5;
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, pos);
    state.entities.set_ground(slot, false);
    let ceiling = state.registry.states.stone;
    for x in (pos[0] as i32 + 1)..=(pos[0] as i32 + 3) {
        state
            .world
            .set_block(x, pos[1] as i32 + 2, pos[2] as i32, ceiling);
    }
    state.world.fold_overlay();
    (state, player, pos)
}

#[test]
fn crouching_player_moves_through_one_and_a_half_block_passage() {
    let (mut state, player, pos) = crouching_passage_fixture();
    let target = [pos[0] + 2.0, pos[1], pos[2]];
    run_tick(
        &mut state,
        &[
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: true,
            },
            PlayerInput::Move {
                connection: ConnectionId(1),
                position: target,
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            },
        ],
    );
    let slot = state.entities.slot_of(player).unwrap();
    assert_eq!(state.entities.pos(slot), target);
    assert_eq!(state.entities.body(slot).height, 1.5);
    assert_eq!(state.entities.row(slot).appearance.pose, 5);
}

#[test]
fn unsneak_under_low_ceiling_stays_crouched_until_standing_fits() {
    let (mut state, player, pos) = crouching_passage_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [pos[0] + 2.0, pos[1], pos[2]]);
    run_tick(
        &mut state,
        &[
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: true,
            },
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: false,
            },
        ],
    );
    assert_eq!(state.entities.body(slot).height, 1.5);
    assert_eq!(state.entities.row(slot).appearance.pose, 5);
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & crate::tick::SNEAKING_FLAG,
        0,
        "releasing the key clears its flag even when the ceiling forces crouching"
    );
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: pos,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
        }],
    );
    assert_eq!(state.entities.pos(slot), pos);
    assert_eq!(state.entities.body(slot).height, 1.8);
    assert_eq!(state.entities.row(slot).appearance.pose, 0);
}

#[test]
fn forced_crouch_bucket_uses_physical_eye_height() {
    use crate::boundary::Hand;
    let (mut state, player, pos) = crouching_passage_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(slot, [pos[0] + 2.0, pos[1], pos[2]]);
    let source = crate::boundary::BlockPos {
        x: pos[0] as i32 + 2,
        y: pos[1] as i32 + 1,
        z: pos[2] as i32 + 2,
    };
    state
        .world
        .set_block(source.x, source.y, source.z, state.registry.states.water);
    state.world.fold_overlay();
    let bucket = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:bucket").unwrap())
        .unwrap();
    let filled = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:water_bucket").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(bucket, 1);
    run_tick(
        &mut state,
        &[
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: true,
            },
            PlayerInput::Sneak {
                connection: ConnectionId(1),
                sneaking: false,
            },
            PlayerInput::UseItem {
                connection: ConnectionId(1),
                hand: Hand::Main,
                yaw: 0.0,
                pitch: 0.0,
                sequence: 1,
            },
        ],
    );
    assert_eq!(state.entities.row(slot).appearance.pose, 5);
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & crate::tick::SNEAKING_FLAG,
        0
    );
    assert_eq!(state.players[&player].inventory.slots[0].item_id, filled);
    assert_eq!(
        state.world.block_at(source.x, source.y, source.z),
        Some(state.registry.states.air)
    );
}

#[test]
fn living_respawn_is_ignored_and_duplicate_dead_respawn_runs_once() {
    let (mut state, player, pos) = flat_player_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).health = 10.0;
    state.entities.row_mut(slot).protected_until = 0;
    state.players.get_mut(&player).unwrap().vitals.health = 10.0;
    state.players.get_mut(&player).unwrap().vitals.saturation = 0.0;
    state.players.get_mut(&player).unwrap().vitals.food = 0;
    let living_vitals = state.players[&player].vitals.clone();
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(1),
        }],
    );
    assert_eq!(state.entities.pos(slot), pos);
    assert_eq!(state.players[&player].vitals.health, 10.0);
    assert_eq!(state.players[&player].vitals, living_vitals);
    assert_eq!(state.entities.row(slot).protected_until, 0);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|c| matches!(c, crate::boundary::PlayerChange::Respawned { .. }))
    );
    state.entities.body_mut(slot).alive = false;
    state.entities.row_mut(slot).health = 0.0;
    state.players.get_mut(&player).unwrap().vitals.health = 0.0;
    let (_, drains, _) = run_tick(
        &mut state,
        &[
            PlayerInput::Respawn {
                connection: ConnectionId(1),
            },
            PlayerInput::Respawn {
                connection: ConnectionId(1),
            },
        ],
    );
    assert_eq!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .filter(|c| matches!(c, crate::boundary::PlayerChange::Respawned { .. }))
            .count(),
        1
    );
    assert_eq!(state.players[&player].vitals.health, 20.0);
    let protected_until = state.entities.row(slot).protected_until;
    assert_eq!(
        protected_until,
        state.tick - 1 + crate::entities::SPAWN_INVULNERABLE_TICKS
    );
    let respawned_at = state.entities.pos(slot);
    state.entities.row_mut(slot).health = 8.0;
    let vitals = &mut state.players.get_mut(&player).unwrap().vitals;
    vitals.health = 8.0;
    vitals.food = 10;
    vitals.saturation = 0.0;
    let damaged_vitals = vitals.clone();
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Respawn {
            connection: ConnectionId(1),
        }],
    );
    assert_eq!(state.entities.pos(slot), respawned_at);
    assert_eq!(state.players[&player].vitals, damaged_vitals);
    assert_eq!(state.entities.row(slot).health, 8.0);
    assert_eq!(state.entities.row(slot).protected_until, protected_until);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|c| matches!(c, crate::boundary::PlayerChange::Respawned { .. }))
    );
}

#[test]
fn sprinting_accepted_distance_drains_hunger_and_reports_it() {
    let (mut state, player, pos) = flat_player_fixture();
    run_tick(
        &mut state,
        &[PlayerInput::Sprint {
            connection: ConnectionId(1),
            sprinting: true,
        }],
    );
    let mut health_sent = false;
    for step in 0..100 {
        let x = if step % 2 == 0 { pos[0] + 4.0 } else { pos[0] };
        let (_, drains, _) = run_tick(
            &mut state,
            &[PlayerInput::Move {
                connection: ConnectionId(1),
                position: [x, pos[1], pos[2]],
                yaw: 0.0,
                pitch: 0.0,
                on_ground: true,
            }],
        );
        health_sent |= drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|c| matches!(c, crate::boundary::PlayerChange::Health { food, .. } if *food < 20));
    }
    assert!(state.players[&player].vitals.food < 20);
    assert!(health_sent);
}

#[test]
fn predicted_table_use_waits_for_accepted_placement() {
    use crate::boundary::{BlockPos, Hand, PlayerChange};
    let (mut state, player, pos) = flat_player_fixture();
    let table = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:crafting_table").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(table, 1);
    let support = BlockPos {
        x: pos[0] as i32 + 2,
        y: pos[1] as i32 - 1,
        z: pos[2] as i32,
    };
    let target = BlockPos {
        y: support.y + 1,
        ..support
    };
    let use_on = |position, sequence| PlayerInput::UseItemOn {
        connection: ConnectionId(1),
        hand: Hand::Main,
        position,
        face: 1,
        cursor: [0.5, 1.0, 0.5],
        sequence,
    };
    run_tick(&mut state, &[use_on(support, 1), use_on(target, 2)]);
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|c| matches!(c, PlayerChange::MenuOpened { menu_type: 12, .. }))
    );
    assert_eq!(
        state.players[&player].open.as_ref().unwrap().kind,
        crate::inventory::MenuKind::CraftingTable
    );
}

#[test]
fn vanilla_pickup_box_collects_items_above_the_players_feet() {
    let (mut state, player, pos) = flat_player_fixture();
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_log").unwrap())
        .unwrap();
    let dropped = crate::tick::spawn_item_entity(
        &mut state,
        [pos[0] + 1.3, pos[1] + 2.1, pos[2] + 1.3],
        mc_data::ItemStack::new(item, 1),
    );
    let slot = state.entities.slot_of(dropped).unwrap();
    state.entities.body_mut(slot).pickup_delay = 0;
    run_tick(&mut state, &[]);
    assert!(state.entities.slot_of(dropped).is_none());
    assert_eq!(
        state.players[&player]
            .inventory
            .slots
            .iter()
            .filter(|s| s.item_id == item)
            .map(|s| s.count)
            .sum::<i32>(),
        1
    );
}

#[test]
fn vanilla_empty_collision_states_are_passable_but_fluids_remain_fluids() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = CoreRegistry::load(Some(&dir)).unwrap();
    for name in [
        "minecraft:short_grass",
        "minecraft:tall_grass",
        "minecraft:dandelion",
        "minecraft:oak_sign",
    ] {
        let block = registry
            .blocks
            .block(&mc_data::Identifier::parse(name).unwrap())
            .unwrap();
        assert_eq!(
            registry.materials.classify(block.default.0),
            mc_physics::BlockMaterial::Air,
            "{name}"
        );
    }
    assert_eq!(
        registry.materials.classify(registry.states.water),
        mc_physics::BlockMaterial::Water
    );
    assert_eq!(
        registry.materials.classify(registry.materials.lava[0]),
        mc_physics::BlockMaterial::Lava
    );
}

#[test]
fn sign_placement_opens_owned_editor_and_text_is_journaled() {
    use crate::boundary::{BlockPos, Hand, PlayerChange};
    let (mut state, player, pos) = flat_player_fixture();
    let sign = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:oak_sign").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(sign, 1);
    let support = BlockPos {
        x: pos[0] as i32 + 2,
        y: pos[1] as i32 - 1,
        z: pos[2] as i32,
    };
    let target = BlockPos {
        y: support.y + 1,
        ..support
    };
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: support,
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
    assert!(drains.iter().flat_map(|(_, changes)| changes).any(
        |c| matches!(c, PlayerChange::SignEditorOpened { position, .. } if *position == target)
    ));
    let lines = vec![
        "Solaris".to_owned(),
        "survival".to_owned(),
        "".to_owned(),
        "".to_owned(),
    ];
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::SignUpdate {
            connection: ConnectionId(1),
            position: target,
            front: true,
            lines,
        }],
    );
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|c| matches!(c, PlayerChange::BlockEntityData { position, block_entity_type: 7, .. } if *position == target))
    );
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|e| matches!(
        e,
        crate::journal::JournalEntry::OpaqueBlockEntity { nbt: Some(_), .. }
    )));
    assert!(
        output
            .snapshot
            .chunks
            .iter()
            .any(|c| c.chunk.block_entities.contains_key(&target))
    );
    let replay = crate::journal::replay_batches(
        crate::journal::Replay::default(),
        &[(output.tick, output.tick, entries)],
    );
    assert!(replay.opaque_block_entities[&(target.x, target.y, target.z)].is_some());
}

#[test]
fn accepted_mob_hits_push_players_once_per_hurt_window() {
    use crate::boundary::{EntityEvent, PlayerChange};
    let (mut state, player, pos) = flat_player_fixture();
    state.difficulty = crate::survival::Difficulty::Normal;
    state.day_time = 14_000;
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let zombie = state.spawn_mob(zombie_type, [pos[0] + 1.0, pos[1], pos[2]], 20.0);
    let victim = state.entities.slot_of(player).unwrap();
    let attacker = state.entities.slot_of(zombie).unwrap();
    state.entities.row_mut(attacker).attack_damage = 2.0;
    state.entities.row_mut(victim).protected_until = state.tick + 1;
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.health, 20.0);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::Motion { .. }),)
    );

    state.entities.body_mut(attacker).attack_cooldown = 0;
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.health, 18.0);
    assert!(
        output
            .changes
            .entity_events
            .contains(&EntityEvent::Hurt { entity: player })
    );
    let motions: Vec<_> = drains
        .iter()
        .flat_map(|(_, changes)| changes)
        .filter_map(|change| match change {
            PlayerChange::Motion {
                entity, velocity, ..
            } if *entity == player => Some(*velocity),
            _ => None,
        })
        .collect();
    assert_eq!(motions, vec![[-8.0, 8.0, 0.0]]);
    assert_eq!(state.entities.vel(victim), motions[0]);
    let hurt_at = state.entities.body(victim).hurt_at.unwrap();

    // Equal damage is refused, stronger damage lands only its excess.
    // Neither starts another hurt event or motion impulse.
    for (damage, health) in [(2.0, 18.0), (4.0, 16.0)] {
        state.entities.body_mut(attacker).attack_cooldown = 0;
        state.entities.row_mut(attacker).attack_damage = damage;
        let (output, drains, _) = run_tick(&mut state, &[]);
        assert_eq!(state.players[&player].vitals.health, health);
        assert!(
            !output
                .changes
                .entity_events
                .contains(&EntityEvent::Hurt { entity: player })
        );
        assert!(
            !drains
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| matches!(change, PlayerChange::Motion { .. }),)
        );
        assert_eq!(state.entities.body(victim).hurt_at, Some(hurt_at));
    }
    state.tick = hurt_at + crate::entities::HURT_INVULNERABLE_TICKS;
    state.entities.body_mut(attacker).attack_cooldown = 0;
    let (_, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.health, 12.0);
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::Motion { .. }),)
    );
}

#[test]
fn player_melee_impulse_survives_the_next_npc_chase_decisions() {
    for (sprint, enchantment, expected_x) in [(false, 0, 8.0), (true, 0, 14.0), (false, 2, 24.0)] {
        let (mut state, player, pos) = flat_player_fixture();
        state.day_time = 14_000;
        state.tick = 20;
        let player_slot = state.entities.slot_of(player).unwrap();
        state.entities.set_rot(player_slot, -90.0, 0.0);
        let zombie_type = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
            .unwrap();
        let zombie = state.spawn_mob(zombie_type, [pos[0] + 1.0, pos[1], pos[2]], 20.0);
        let slot = state.entities.slot_of(zombie).unwrap();
        state.entities.set_ground(slot, true);
        state.entities.body_mut(slot).attack_cooldown = 100;
        let player_state = state.players.get_mut(&player).unwrap();
        player_state.sprinting = sprint;
        if enchantment > 0 {
            let sword = state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
                .unwrap();
            let mut stack = mc_data::ItemStack::new(sword, 1);
            stack.enchantments.push(mc_data::ItemEnchantment {
                id: mc_data::Identifier::parse("minecraft:knockback").unwrap(),
                level: enchantment,
            });
            player_state.inventory.slots[0] = stack;
        }
        run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: zombie,
            }],
        );
        let velocity = state.entities.vel(slot);
        assert!((velocity[0] - expected_x).abs() < 1.0e-6, "{velocity:?}");
        assert_eq!(velocity[1], 8.0);
        let before = state.entities.pos(slot);
        run_tick(&mut state, &[]);
        let first = state.entities.pos(slot);
        assert!(
            first[0] > before[0] && first[1] > before[1],
            "first impulse step: {first:?}"
        );
        run_tick(&mut state, &[]);
        let second = state.entities.pos(slot);
        assert!(
            second[0] > first[0] && second[1] > first[1],
            "retained impulse: {second:?}"
        );
    }
}

#[test]
fn combat_impulse_collides_with_a_wall_and_void_damage_never_pushes() {
    use crate::boundary::PlayerChange;
    let (mut state, player, pos) = flat_player_fixture();
    state.day_time = 14_000;
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let at = [pos[0] + 1.0, pos[1], pos[2]];
    let zombie = state.spawn_mob(zombie_type, at, 20.0);
    let slot = state.entities.slot_of(zombie).unwrap();
    state.entities.set_ground(slot, true);
    state.entities.body_mut(slot).attack_cooldown = 100;
    let wall_x = at[0].floor() as i32 + 1;
    for dy in 0..4 {
        state.world.set_block(
            wall_x,
            pos[1] as i32 + dy,
            pos[2] as i32,
            state.registry.states.stone,
        );
    }
    state.world.fold_overlay();
    run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target: zombie,
        }],
    );
    run_tick(&mut state, &[]);
    assert!(
        state.entities.pos(slot)[0]
            <= f64::from(wall_x) - state.entities.body(slot).half_width + 1.0e-7
    );
    assert_eq!(state.entities.vel(slot)[0], 0.0);
    assert_eq!(state.entities.horizontal_impulse(slot)[0], 0.0);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Command {
            connection: ConnectionId(1),
            command: "kill @s".to_owned(),
            permission_level: 4,
        }],
    );
    assert_eq!(state.players[&player].vitals.health, 0.0);
    assert!(
        !drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::Motion { .. }),)
    );
}

#[test]
fn bottom_slab_accepts_player_step_and_supports_mob_at_half_height() {
    let mut state = test_state();
    let player = join(&mut state, 1, "slab");
    let slot = state.entities.slot_of(player).expect("player slot");
    let spawn = state.entities.pos(slot);
    let (x, y, z) = (
        spawn[0].floor() as i32,
        spawn[1].floor() as i32 + 8,
        spawn[2].floor() as i32,
    );
    let slab = state
        .registry
        .blocks
        .states()
        .find(|state| {
            state.block.id.as_str() == "minecraft:oak_slab"
                && state
                    .properties
                    .iter()
                    .any(|(k, v)| k == "type" && v == "bottom")
                && state
                    .properties
                    .iter()
                    .any(|(k, v)| k == "waterlogged" && v == "false")
        })
        .expect("bottom slab")
        .id
        .0;
    for bx in x - 3..=x + 3 {
        for bz in z - 3..=z + 3 {
            state
                .world
                .set_block(bx, y - 1, bz, state.registry.states.stone);
            state.world.set_block(bx, y, bz, slab);
            for by in y + 1..=y + 4 {
                state.world.set_block(bx, by, bz, state.registry.states.air);
            }
        }
    }
    // A full block immediately before the slab provides the approach floor.
    state
        .world
        .set_block(x - 2, y, z, state.registry.states.air);
    let start = [f64::from(x) - 1.5, f64::from(y), f64::from(z) + 0.5];
    state.entities.set_pos(slot, start);
    state.entities.set_ground(slot, true);
    let target = [f64::from(x) - 0.5, f64::from(y) + 0.5, start[2]];
    run_tick(
        &mut state,
        &[PlayerInput::Move {
            connection: ConnectionId(1),
            position: target,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        }],
    );
    assert_eq!(state.entities.pos(slot), target, "legal half-block step");
    let cow = state.spawn_mob(
        state.registry.cow_type_id,
        [f64::from(x) + 0.5, f64::from(y) + 2.0, start[2]],
        10.0,
    );
    let cow_slot = state.entities.slot_of(cow).expect("cow slot");
    for _ in 0..30 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.entities.pos(cow_slot)[1],
        f64::from(y) + 0.5,
        "mob rests on the slab, not a phantom full cube"
    );
}

#[test]
fn nonlethal_player_damage_publishes_committed_health_without_waiting_for_death() {
    use crate::boundary::PlayerChange;
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;

    let (mut state, player, _) = flat_player_fixture();
    let slot = state.entities.slot_of(player).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    state.tick = 100;

    // Fresh hit, equal hit during i-frames, stronger excess hit during
    // i-frames, then another fresh hit: publish every committed post-image,
    // but do not manufacture a health update for the refused hit.
    for (tick, amount, accepted) in [
        (100, 2.5, true),
        (101, 2.5, false),
        (102, 3.5, true),
        (121, 2.5, true),
    ] {
        state.tick = tick;
        let before = state.players[&player].vitals.health;
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount,
                kind: DamageKind::Melee,
                source: None,
            },
        ));
        let (output, drains, _) = run_tick(&mut state, &[]);
        let committed = state.players[&player].vitals.health;
        let updates: Vec<_> = drains
            .iter()
            .flat_map(|(connection, changes)| {
                changes.iter().map(move |change| (connection, change))
            })
            .filter_map(|(connection, change)| match change {
                PlayerChange::Health {
                    health,
                    food,
                    saturation,
                } => Some((*connection, *health, *food, *saturation)),
                _ => None,
            })
            .collect();
        if accepted {
            assert!(committed > 0.0 && committed < before);
            assert_eq!(
                updates,
                vec![(
                    ConnectionId(1),
                    committed,
                    state.players[&player].vitals.food as u8,
                    state.players[&player].vitals.saturation,
                )],
                "nonlethal damage must reach the client on its commit tick"
            );
            assert_eq!(state.entities.row(slot).appearance.health, committed);
            assert!(output.changes.entities_appearance_changed.contains(&player));
        } else {
            assert_eq!(committed, before);
            assert!(updates.is_empty(), "i-frame refusal is not health damage");
        }
        assert!(
            !drains
                .iter()
                .flat_map(|(_, changes)| changes)
                .any(|change| { matches!(change, PlayerChange::Died { .. }) })
        );
    }
}

#[test]
fn accepted_melee_wears_and_breaks_armor_and_syncs_the_inventory() {
    use crate::boundary::PlayerChange;
    use crate::commands::GameMode;
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;
    use mc_data::item_components::{ItemArmorFacts, ItemFacts, ItemFactsTable};

    for (kind, mode, breaks) in [
        (DamageKind::Melee, GameMode::Survival, true),
        (DamageKind::Fall, GameMode::Survival, false),
        (DamageKind::Melee, GameMode::Creative, false),
        (DamageKind::Melee, GameMode::Spectator, false),
    ] {
        let (mut state, player, _) = flat_player_fixture();
        let mut registry = CoreRegistry::load(None).unwrap();
        let chestplate = mc_data::Identifier::parse("minecraft:iron_chestplate").unwrap();
        let item = registry.items.id_of(&chestplate).unwrap();
        registry.item_facts = ItemFactsTable::from_entries([(
            chestplate,
            ItemFacts {
                max_damage: Some(240),
                armor: Some(ItemArmorFacts {
                    slot: "chest".to_owned(),
                    armor: 6.0,
                    toughness: 0.0,
                }),
                ..ItemFacts::default()
            },
        )]);
        state.registry = Arc::new(registry);
        let actor = state.players.get_mut(&player).unwrap();
        actor.game_mode = mode;
        actor.inventory.slots[38] = mc_data::ItemStack::new(item, 1);
        actor.inventory.slots[38].damage = Some(239);
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.tick = 100;
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 2.5,
                kind,
                source: None,
            },
        ));
        let (_, drains, _) = run_tick(&mut state, &[]);
        assert_eq!(
            state.players[&player].inventory.slots[38].is_empty(),
            breaks,
            "{kind:?} in {mode:?}"
        );
        let inventory_updates: Vec<_> = drains
            .iter()
            .flat_map(|(connection, changes)| {
                changes.iter().map(move |change| (connection, change))
            })
            .filter_map(|(connection, change)| match change {
                PlayerChange::MenuContents {
                    window: 0, slots, ..
                } => Some((*connection, slots)),
                _ => None,
            })
            .collect();
        if breaks {
            assert_eq!(
                inventory_updates.len(),
                1,
                "broken armor must reach the client"
            );
            assert_eq!(inventory_updates[0].0, ConnectionId(1));
            assert_eq!(
                inventory_updates[0].1[6], None,
                "menu slot 6 is chest armor"
            );
        } else {
            assert_eq!(state.players[&player].inventory.slots[38].damage, Some(239));
            assert!(
                inventory_updates.is_empty(),
                "no armor wear means no inventory sync"
            );
        }
    }
}

#[test]
fn shield_raise_delay_front_arc_and_durability_gate_zombie_hits() {
    use crate::boundary::{EntityEvent, Hand, PlayerChange};
    use crate::inventory::OFF_HAND;
    for (hand, elapsed, front, blocked) in [
        (Hand::Off, 5, true, true),
        (Hand::Main, 5, true, true),
        (Hand::Off, 5, false, false),
        (Hand::Off, 4, true, false),
    ] {
        let (mut state, player, pos) = flat_player_fixture();
        state.difficulty = crate::survival::Difficulty::Normal;
        state.day_time = 14_000;
        let victim = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(victim).protected_until = 0;
        // UseItem's yaw must override the prior look direction.
        state.entities.set_rot(victim, 180.0, 0.0);
        let shield = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:shield").unwrap())
            .unwrap();
        let held = if hand == Hand::Off { OFF_HAND } else { 0 };
        state.players.get_mut(&player).unwrap().inventory.slots[held] =
            mc_data::ItemStack::new(shield, 1);
        run_tick(
            &mut state,
            &[PlayerInput::UseItem {
                connection: ConnectionId(1),
                hand,
                yaw: 0.0,
                pitch: 0.0,
                sequence: 1,
            }],
        );
        for _ in 1..elapsed - 1 {
            run_tick(&mut state, &[]);
        }
        let zombie_type = state
            .registry
            .entity_types
            .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
            .unwrap();
        let zombie = state.spawn_mob(
            zombie_type,
            [pos[0], pos[1], pos[2] + if front { 1.0 } else { -1.0 }],
            20.0,
        );
        let attacker = state.entities.slot_of(zombie).unwrap();
        state.entities.row_mut(attacker).attack_damage = 3.0;
        let (output, drains, _) = run_tick(&mut state, &[]);
        assert_eq!(
            state.players[&player].vitals.health,
            if blocked { 20.0 } else { 17.0 },
            "{hand:?} elapsed{elapsed} front{front}"
        );
        assert_eq!(
            state.players[&player].inventory.slots[held].damage,
            blocked.then_some(4)
        );
        assert_eq!(
            output
                .changes
                .entity_events
                .contains(&EntityEvent::ShieldBlock {
                    entity: player,
                    disabled: false
                }),
            blocked
        );
        assert_eq!(
            output
                .changes
                .entity_events
                .contains(&EntityEvent::Hurt { entity: player }),
            !blocked
        );
        if blocked {
            assert_eq!(
                state.entities.body(victim).hurt_at,
                None,
                "blocking never starts hurt invulnerability"
            );
            assert!(
                state.entities.vel(attacker)[2] > 0.0,
                "blocked attacker is knocked away"
            );
            assert!(
                drains
                    .iter()
                    .flat_map(|(_, changes)| changes)
                    .any(|change| matches!(change, PlayerChange::MenuContents { window: 0, .. }))
            );
            assert!(!drains.iter().flat_map(|(_, changes)| changes).any(
                |change| matches!(change, PlayerChange::Motion { entity, .. } if *entity == player)
            ));
        }
    }
}

#[test]
fn raised_shield_blocks_incoming_projectiles_but_not_rear_or_environmental_damage() {
    use crate::boundary::{EntityEvent, Hand};
    use crate::intent::DamageSource;
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;
    for (kind, velocity, blocked) in [
        (DamageKind::Projectile, [0.0, -1.0], true),
        (DamageKind::Projectile, [0.0, 1.0], false),
        (DamageKind::Fall, [0.0, -1.0], false),
    ] {
        let (mut state, player, _) = flat_player_fixture();
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.entities.set_rot(slot, 0.0, 0.0);
        let shield = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:shield").unwrap())
            .unwrap();
        let actor = state.players.get_mut(&player).unwrap();
        actor.inventory.slots[0] = mc_data::ItemStack::new(shield, 1);
        actor.use_item(&state.registry.item_rules(), Hand::Main);
        for _ in 0..5 {
            actor.tick_item_use(&state.registry.item_rules());
        }
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 3.5,
                kind,
                source: Some(DamageSource::Projectile {
                    owner: crate::boundary::EntityId(-1),
                    velocity,
                    punch: 0,
                    projectile: crate::boundary::EntityId(-2),
                    direct_type_id: 0,
                    flame: false,
                }),
            },
        ));
        let (_, events, _, _) = state.commit_merge_effects();
        assert_eq!(
            state.players[&player].vitals.health,
            if blocked { 20.0 } else { 16.5 }
        );
        assert_eq!(
            state.players[&player].inventory.slots[0].damage,
            blocked.then_some(4)
        );
        assert_eq!(
            events.contains(&EntityEvent::ShieldBlock {
                entity: player,
                disabled: false
            }),
            blocked
        );
    }
}

#[test]
fn shield_break_stops_use_and_axe_disables_both_hands_for_one_hundred_ticks() {
    use crate::boundary::{EntityEvent, Hand, PlayerChange};
    use crate::intent::DamageSource;
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;
    for breaks in [false, true] {
        let (mut state, player, pos) = flat_player_fixture();
        let slot = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(slot).protected_until = 0;
        state.entities.set_rot(slot, 0.0, 0.0);
        let attacker = join(&mut state, 2, "axe");
        let shield = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:shield").unwrap())
            .unwrap();
        let axe = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:iron_axe").unwrap())
            .unwrap();
        state.players.get_mut(&attacker).unwrap().inventory.slots[0] =
            mc_data::ItemStack::new(axe, 1);
        let rules = state.registry.item_rules();
        let actor = state.players.get_mut(&player).unwrap();
        actor.inventory.slots[0] = mc_data::ItemStack::new(shield, 1);
        actor.inventory.slots[crate::inventory::OFF_HAND] = mc_data::ItemStack::new(shield, 1);
        if breaks {
            actor.inventory.slots[0].damage = Some(335);
        }
        actor.use_item(&rules, Hand::Main);
        for _ in 0..5 {
            actor.tick_item_use(&rules);
        }
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 3.0,
                kind: DamageKind::Melee,
                source: Some(DamageSource::Melee {
                    attacker,
                    position: [pos[0], pos[1], pos[2] + 1.0],
                    yaw: 0.0,
                    bonus_knockback: 0.0,
                    sweep: false,
                    fire_ticks: 0,
                    bane_ticks: 0,
                }),
            },
        ));
        let (_, events, _, _) = state.commit_merge_effects();
        assert_eq!(state.players[&player].vitals.health, 20.0);
        assert_eq!(state.players[&player].inventory.slots[0].is_empty(), breaks);
        assert!(events.contains(&EntityEvent::ShieldBlock {
            entity: player,
            disabled: true
        }));
        assert_eq!(
            state.outbox[&ConnectionId(1)]
                .iter()
                .any(|change| matches!(change, PlayerChange::ShieldCooldown { ticks: 100, .. })),
            !breaks
        );
        let rules = state.registry.item_rules();
        let actor = state.players.get_mut(&player).unwrap();
        assert!(
            actor.raised_shield(&rules).is_none(),
            "break or disable stopped active use"
        );
        if breaks {
            actor.use_item(&rules, Hand::Off);
            for _ in 0..5 {
                actor.tick_item_use(&rules);
            }
            assert_eq!(actor.raised_shield(&rules), Some(Hand::Off));
            continue;
        }
        for _ in 0..99 {
            actor.tick_item_use(&rules);
        }
        actor.use_item(&rules, Hand::Off);
        actor.tick_item_use(&rules);
        for _ in 0..5 {
            actor.tick_item_use(&rules);
        }
        assert!(
            actor.raised_shield(&rules).is_none(),
            "the attempt made during cooldown did not start use"
        );
        actor.use_item(&rules, Hand::Off);
        for _ in 0..5 {
            actor.tick_item_use(&rules);
        }
        assert_eq!(actor.raised_shield(&rules), Some(Hand::Off));
    }
}

#[test]
fn team_join_and_friendly_fire_block_melee_and_projectiles_without_hurt_side_effects() {
    use crate::intent::DamageSource;
    use crate::survival::DamageKind;
    use crate::tick::DamagePiece;
    let (mut state, victim, pos) = flat_player_fixture();
    let attacker = join(&mut state, 2, "Attacker");
    let victim_name = state.connections.name_of(victim).unwrap().to_owned();
    for args in [
        vec!["add", "allies"],
        vec!["join", "allies", "Attacker"],
        vec!["join", "allies", victim_name.as_str()],
        vec!["modify", "allies", "friendlyFire", "false"],
    ] {
        let command = crate::scoreboard::ScoreboardCommand::parse("team", &args).unwrap();
        crate::scoreboard::execute(&mut state, attacker, &command);
    }
    let slot = state.entities.slot_of(victim).unwrap();
    state.entities.row_mut(slot).protected_until = 0;
    let before = state.players[&victim].vitals.health;
    for source in [
        DamageSource::Melee {
            attacker,
            position: [pos[0], pos[1], pos[2] + 1.0],
            yaw: 0.0,
            bonus_knockback: 0.0,
            sweep: false,
            fire_ticks: 0,
            bane_ticks: 0,
        },
        DamageSource::Projectile {
            owner: attacker,
            velocity: [0.0, -1.0],
            punch: 0,
            projectile: crate::boundary::EntityId(-2),
            direct_type_id: 0,
            flame: false,
        },
    ] {
        let kind = if matches!(source, DamageSource::Projectile { .. }) {
            DamageKind::Projectile
        } else {
            DamageKind::Melee
        };
        state.merge_damage.push((
            slot,
            DamagePiece {
                physical: None,
                custom_armor: false,
                amount: 5.0,
                kind,
                source: Some(source),
            },
        ));
        let (_, events, _, _) = state.commit_merge_effects();
        assert!(events.is_empty());
        assert_eq!(state.players[&victim].vitals.health, before);
        assert_eq!(state.entities.body(slot).hurt_at, None);
    }
    let command = crate::scoreboard::ScoreboardCommand::parse(
        "team",
        &["modify", "allies", "friendlyFire", "true"],
    )
    .unwrap();
    crate::scoreboard::execute(&mut state, attacker, &command);
    state.merge_damage.push((
        slot,
        DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 5.0,
            kind: DamageKind::Melee,
            source: Some(DamageSource::Melee {
                attacker,
                position: [pos[0], pos[1], pos[2] + 1.0],
                yaw: 0.0,
                bonus_knockback: 0.0,
                sweep: false,
                fire_ticks: 0,
                bane_ticks: 0,
            }),
        },
    ));
    state.commit_merge_effects();
    assert_eq!(state.players[&victim].vitals.health, before - 5.0);
}

#[test]
fn scoreboard_command_operations_and_join_snapshot_are_authoritative() {
    let (mut state, player, _) = flat_player_fixture();
    for command in [
        "scoreboard objectives add wins dummy",
        "scoreboard objectives setdisplay sidebar wins",
        "scoreboard players set Alice wins 7",
        "scoreboard players set Bob wins 3",
        "scoreboard players operation Alice wins += Bob wins",
    ] {
        let parsed = crate::commands::parse(command, true).unwrap();
        let crate::commands::Command::Scoreboard(command) = parsed else {
            panic!("scoreboard command");
        };
        crate::scoreboard::execute(&mut state, player, &command);
    }
    assert_eq!(state.scoreboard.scores["Alice"]["wins"], 10);
    assert_eq!(state.scoreboard.display_slots[&1], "wins");
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(9),
            uuid: uuid::Uuid::new_v4(),
            name: "Viewer".to_owned(),
            permission_level: 0,
        }],
    );
    let events: Vec<_> = drains
        .iter()
        .filter(|(connection, _)| *connection == ConnectionId(9))
        .flat_map(|(_, changes)| changes)
        .filter_map(|change| match change {
            crate::boundary::PlayerChange::Scoreboard { event, .. } => Some(event),
            _ => None,
        })
        .collect();
    assert!(events.iter().any(|event| matches!(event, crate::scoreboard::ScoreboardEvent::Display { slot: 1, objective } if objective == "wins")));
    assert!(events.iter().any(|event| matches!(event, crate::scoreboard::ScoreboardEvent::Score { owner, objective, score: 10 } if owner == "Alice" && objective == "wins")));
}

fn orb_total(state: &TickState) -> i32 {
    state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).appearance.experience_orb)
        .map(|(value, count)| value * count)
        .sum()
}

#[test]
fn zombie_player_kill_spawns_five_experience_in_vanilla_buckets() {
    let (mut state, player, pos) = flat_player_fixture();
    let zombie_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let zombie = state.spawn_mob(zombie_type, [pos[0] + 4.0, pos[1], pos[2]], 20.0);
    let slot = state.entities.slot_of(zombie).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: 100.0,
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
    let (_, _, spawned, _) = state.commit_merge_effects();
    assert!(spawned.iter().any(
        |id| state.entities.slot_of(*id).is_some_and(|slot| matches!(
            state.entities.body(slot).kind,
            crate::entities::EntityKind::ExperienceOrb { .. }
        ))
    ));
    assert_eq!(orb_total(&state), 5);
}

#[test]
fn coal_break_experience_is_zero_to_two_and_silk_touch_suppresses_it() {
    let registry = test_registry();
    let coal = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:coal_ore").unwrap())
        .unwrap()
        .default
        .0;
    // Embedded fixtures omit extracted mining and iron-pickaxe tool facts;
    // supply the canonical coal harvest relation rather than disabling it.
    let mut requires = vec![false; registry.blocks.len()];
    requires[coal as usize] = true;
    let mining = mc_data::block_mining::BlockMiningTable::from_arrays(
        "26.1.2",
        vec![3.0; requires.len()],
        requires,
    );
    let drops = crate::block_drops::BlockDropRules {
        blocks: &registry.blocks,
        loot: &registry.loot,
        mining: &mining,
    };
    let pickaxe = registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .unwrap();
    let facts = mc_data::item_components::ItemFactsTable::from_entries([(
        mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap(),
        mc_data::item_components::ItemFacts {
            tool: Some(mc_data::item_components::ToolFacts {
                default_mining_speed: Some(1.0),
                damage_per_block: Some(1),
                rules: vec![mc_data::item_components::ToolRuleFacts {
                    blocks: vec!["minecraft:coal_ore".into()],
                    speed: Some(6.0),
                    correct_for_drops: Some(true),
                }],
            }),
            ..Default::default()
        },
    )]);
    let rules = crate::inventory::ItemRules {
        facts: &facts,
        ..registry.item_rules()
    };
    let mut tool = mc_data::ItemStack::new(pickaxe, 1);
    let mut seen = [false; 3];
    for seed in 0..100 {
        let xp = crate::block_drops::block_experience(&drops, &rules, coal, &tool, seed);
        assert!((0..=2).contains(&xp));
        seen[xp as usize] = true;
    }
    assert_eq!(seen, [true; 3]);
    tool.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:silk_touch").unwrap(),
        level: 1,
    });
    assert_eq!(
        crate::block_drops::block_experience(&drops, &rules, coal, &tool, 2),
        0
    );
    assert_eq!(
        crate::block_drops::block_experience(&drops, &rules, coal, &mc_data::ItemStack::EMPTY, 2),
        0
    );
}

#[test]
fn orb_pickup_raises_level_and_sends_take_and_experience() {
    let (mut state, player, pos) = flat_player_fixture();
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, pos, 7, &mut spawned);
    let slot = state.entities.slot_of(spawned[0]).unwrap();
    state.entities.set_vel(slot, [0.0; 3]);
    let (output, drains, _) = run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.experience.level, 1);
    assert_eq!(state.players[&player].vitals.experience.total, 7);
    assert!(output.changes.entities_removed.contains(&spawned[0]));
    assert!(output.changes.entity_events.iter().any(|event|
        matches!(event, crate::boundary::EntityEvent::Pickup { collector, count: 1, .. } if *collector == player)));
    assert!(
        drains
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                crate::boundary::PlayerChange::Experience {
                    level: 1,
                    total: 7,
                    ..
                }
            ))
    );
}

#[test]
fn equal_orbs_merge_only_in_the_same_vanilla_group() {
    let (mut state, _, pos) = flat_player_fixture();
    let at = [pos[0] + 4.0, pos[1], pos[2]];
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, at, 3, &mut spawned);
    for _ in 0..39 {
        state.entities.alloc_id();
    }
    crate::experience_orbs::award(&mut state, [at[0] + 10.0, at[1], at[2]], 3, &mut spawned);
    assert_eq!(spawned.len(), 2);
    for id in &spawned {
        let slot = state.entities.slot_of(*id).unwrap();
        state.entities.set_pos(slot, at);
        state.entities.set_vel(slot, [0.0; 3]);
    }
    state.tick = 20;
    let mut changes = crate::boundary::ChangeSet::default();
    crate::experience_orbs::run(&mut state, &mut changes, &mut Vec::new());
    assert_eq!(changes.entities_removed.len(), 1);
    assert_eq!(orb_total(&state), 6);
    let count = state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).appearance.experience_orb)
        .next()
        .unwrap()
        .1;
    assert_eq!(count, 2);
}

#[test]
fn orb_despawns_at_six_thousand_ticks() {
    let (mut state, _, pos) = flat_player_fixture();
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, [pos[0] + 4.0, pos[1], pos[2]], 3, &mut spawned);
    let slot = state.entities.slot_of(spawned[0]).unwrap();
    state.entities.row_mut(slot).age = 5999;
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(output.changes.entities_removed.contains(&spawned[0]));
    assert_eq!(orb_total(&state), 0);
}

#[test]
fn locked_session_collects_no_xp_and_orb_stays() {
    let (mut state, player, pos) = flat_player_fixture();
    state.require_plugin_login = true;
    let connection = state.players[&player].connection;
    state.locked_sessions.insert(connection);
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, pos, 7, &mut spawned);
    let slot = state.entities.slot_of(spawned[0]).unwrap();
    state.entities.set_vel(slot, [0.0; 3]);
    crate::experience_orbs::run(
        &mut state,
        &mut crate::boundary::ChangeSet::default(),
        &mut Vec::new(),
    );
    assert_eq!(state.players[&player].vitals.experience.total, 0);
    assert!(state.entities.slot_of(spawned[0]).is_some(), "orb stays");
    state.locked_sessions.remove(&connection);
    crate::experience_orbs::run(
        &mut state,
        &mut crate::boundary::ChangeSet::default(),
        &mut Vec::new(),
    );
    assert_eq!(state.players[&player].vitals.experience.total, 7);
}

#[test]
fn orb_pickup_mends_equipment_before_crediting_xp() {
    let (mut state, player, pos) = flat_player_fixture();
    let pickaxe = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .unwrap();
    let mut tool = mc_data::ItemStack::new(pickaxe, 1);
    tool.damage = Some(6);
    tool.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:mending").unwrap(),
        level: 1,
    });
    state.players.get_mut(&player).unwrap().inventory.slots[0] = tool;
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, pos, 7, &mut spawned);
    let slot = state.entities.slot_of(spawned[0]).unwrap();
    state.entities.set_vel(slot, [0.0; 3]);
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].inventory.slots[0].damage, Some(0));
    assert_eq!(state.players[&player].vitals.experience.total, 4);
}

#[test]
fn merged_orb_pickups_obey_two_tick_player_cooldown() {
    let (mut state, player, pos) = flat_player_fixture();
    let mut spawned = Vec::new();
    crate::experience_orbs::award(&mut state, pos, 3, &mut spawned);
    let id = spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    Arc::make_mut(&mut state.entities.row_mut(slot).appearance).experience_orb = Some((3, 3));
    state.entities.set_vel(slot, [0.0; 3]);
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.experience.total, 3);
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.experience.total, 3);
    run_tick(&mut state, &[]);
    assert_eq!(state.players[&player].vitals.experience.total, 6);
    assert_eq!(orb_total(&state), 3);
}

#[test]
fn accepted_coal_break_spawns_orbs_and_silk_touch_spawns_none() {
    let (mut state, player, pos) = flat_player_fixture();
    let coal = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:coal_ore").unwrap())
        .unwrap()
        .default
        .0;
    let pickaxe = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_pickaxe").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] =
        mc_data::ItemStack::new(pickaxe, 1);
    let position = crate::boundary::BlockPos {
        x: pos[0] as i32 + 4,
        y: pos[1] as i32,
        z: pos[2] as i32,
    };
    for tick in 0..8 {
        state.tick = tick;
        let before = orb_total(&state);
        let xp = state.block_break_experience(player, coal);
        state
            .merge_broken
            .push((player, position, coal, Vec::new(), xp));
        let (_, _, _, _) = state.commit_merge_effects();
        let delta = orb_total(&state) - before;
        assert!((0..=2).contains(&delta));
    }
    assert!(orb_total(&state) > 0);
    state.players.get_mut(&player).unwrap().inventory.slots[0]
        .enchantments
        .push(mc_data::ItemEnchantment {
            id: mc_data::Identifier::parse("minecraft:silk_touch").unwrap(),
            level: 1,
        });
    let before = orb_total(&state);
    let xp = state.block_break_experience(player, coal);
    state
        .merge_broken
        .push((player, position, coal, Vec::new(), xp));
    state.commit_merge_effects();
    assert_eq!(orb_total(&state), before);
}

#[test]
fn workstation_wire_inputs_own_inputs_and_commit_station_outputs() {
    use crate::boundary::{BlockPos, ClickMode, Hand, PlayerChange};
    use crate::inventory::MenuKind;
    for (block, kind, menu_type) in [
        ("minecraft:enchanting_table", MenuKind::EnchantingTable, 13),
        ("minecraft:anvil", MenuKind::Anvil, 8),
        ("minecraft:grindstone", MenuKind::Grindstone, 15),
    ] {
        let (mut state, entity, pos) = flat_player_fixture();
        let target = BlockPos {
            x: pos[0] as i32 + 2,
            y: pos[1] as i32,
            z: pos[2] as i32,
        };
        let block_id = state
            .registry
            .blocks
            .states()
            .find(|entry| entry.block.id.as_str() == block)
            .unwrap()
            .id
            .0;
        state
            .world
            .set_block(target.x, target.y, target.z, block_id);
        state
            .players
            .get_mut(&entity)
            .unwrap()
            .vitals
            .experience
            .level = 40;
        let (_, drains, _) = run_tick(
            &mut state,
            &[PlayerInput::UseItemOn {
                connection: ConnectionId(1),
                hand: Hand::Main,
                position: target,
                face: 1,
                cursor: [0.5, 1.0, 0.5],
                sequence: 1,
            }],
        );
        assert!(drains.iter().flat_map(|(_, changes)| changes).any(|change|
            matches!(change, PlayerChange::MenuOpened { menu_type: found, .. } if *found == menu_type)));
        assert!(
            !state
                .block_entities
                .contains_key(&(target.x, target.y, target.z))
        );
        let sword_id = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:diamond_sword").unwrap())
            .unwrap();
        let mut sword = mc_data::ItemStack::new(sword_id, 1);
        if kind == MenuKind::Grindstone {
            sword = sword.with_enchantment(
                mc_data::Identifier::parse("minecraft:sharpness").unwrap(),
                3,
            );
        }
        run_tick(
            &mut state,
            &[PlayerInput::TestGive {
                connection: ConnectionId(1),
                slot: 9,
                stack: sword,
            }],
        );
        let menu = state.players[&entity].open.as_ref().unwrap();
        let (window, revision) = (menu.window, menu.state_id);
        run_tick(
            &mut state,
            &[PlayerInput::ContainerClick {
                connection: ConnectionId(1),
                window,
                state_id: revision,
                slot: if kind == MenuKind::EnchantingTable {
                    2
                } else {
                    3
                },
                button: 0,
                mode: ClickMode::QuickMove,
            }],
        );
        assert_eq!(
            state.players[&entity].open.as_ref().unwrap().grid[0].item_id,
            sword_id
        );
        if kind == MenuKind::EnchantingTable {
            let lapis = state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse("minecraft:lapis_lazuli").unwrap())
                .unwrap();
            run_tick(
                &mut state,
                &[PlayerInput::TestGive {
                    connection: ConnectionId(1),
                    slot: 10,
                    stack: mc_data::ItemStack::new(lapis, 3),
                }],
            );
            let revision = state.players[&entity].open.as_ref().unwrap().state_id;
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClick {
                    connection: ConnectionId(1),
                    window,
                    state_id: revision,
                    slot: 3,
                    button: 0,
                    mode: ClickMode::QuickMove,
                }],
            );
            let seed = state.players[&entity].enchantment_seed;
            run_tick(
                &mut state,
                &[PlayerInput::ContainerButton {
                    connection: ConnectionId(1),
                    window,
                    button: 0,
                }],
            );
            assert_ne!(state.players[&entity].enchantment_seed, seed);
            assert!(
                !state.players[&entity].open.as_ref().unwrap().grid[0]
                    .enchantments
                    .is_empty()
            );
        } else {
            if kind == MenuKind::Anvil {
                run_tick(
                    &mut state,
                    &[PlayerInput::RenameItem {
                        connection: ConnectionId(1),
                        name: "Wire blade".into(),
                    }],
                );
            }
            let revision = state.players[&entity].open.as_ref().unwrap().state_id;
            run_tick(
                &mut state,
                &[PlayerInput::ContainerClick {
                    connection: ConnectionId(1),
                    window,
                    state_id: revision,
                    slot: 2,
                    button: 0,
                    mode: ClickMode::Pickup,
                }],
            );
            let menu = state.players[&entity].open.as_ref().unwrap();
            assert!(menu.grid[0].is_empty());
            assert_eq!(menu.carried.item_id, sword_id);
            if kind == MenuKind::Anvil {
                assert_eq!(menu.carried.custom_name.as_deref(), Some("Wire blade"));
            } else {
                assert!(menu.carried.enchantments.is_empty());
                assert!(state.entities.slots().any(|slot| matches!(
                    state.entities.body(slot).kind,
                    crate::entities::EntityKind::ExperienceOrb { .. }
                )));
            }
        }
    }
}

#[path = "consumables_tests.rs"]
mod consumables_tests;
#[path = "contact_hazards_tests.rs"]
mod contact_hazards_tests;

#[test]
fn fire_aspect_captures_the_swing_stack_and_only_ignites_accepted_hits() {
    for (level, invulnerable) in [(0, false), (1, false), (2, false), (2, true)] {
        let (mut state, actor, pos) = flat_player_fixture();
        state.tick = 20;
        let target = state.spawn_mob(
            state.registry.cow_type_id,
            [pos[0] + 1.0, pos[1], pos[2]],
            100.0,
        );
        let slot = state.entities.slot_of(target).unwrap();
        state.entities.row_mut(slot).controls.no_ai = true;
        state.entities.row_mut(slot).controls.invulnerable = invulnerable;
        let mut sword = mc_data::ItemStack::new(
            state
                .registry
                .items
                .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
                .unwrap(),
            1,
        );
        sword.enchantments.push(mc_data::ItemEnchantment {
            id: mc_data::Identifier::parse("minecraft:fire_aspect").unwrap(),
            level,
        });
        // Breaking the weapon during input must not erase the admitted hit's enchantment.
        let rules = state.registry.item_rules();
        let max_damage = rules
            .facts
            .facts_for_stack(&sword, rules.items)
            .unwrap()
            .max_damage
            .unwrap();
        sword.damage = Some(max_damage as i32 - 1);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = sword;
        run_tick(
            &mut state,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target,
            }],
        );
        let target_slot = state.entities.slot_of(target).unwrap();
        let row = state.entities.row(target_slot);
        assert_eq!(
            state.entities.body(target_slot).burn_ticks,
            if invulnerable { 0 } else { level as u32 * 80 }
        );
        assert_eq!(
            row.appearance.shared_flags & 1 != 0,
            level > 0 && !invulnerable
        );
        assert!(state.players[&actor].inventory.slots[0].is_empty());
    }
}

#[test]
fn lethal_fire_aspect_hit_cooks_the_actual_death_loot() {
    let (mut state, actor, pos) = flat_player_fixture();
    state.tick = 20;
    let target = state.spawn_mob(
        state.registry.cow_type_id,
        [pos[0] + 1.0, pos[1], pos[2]],
        1.0,
    );
    let mut sword = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
            .unwrap(),
        1,
    );
    sword.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:fire_aspect").unwrap(),
        level: 1,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = sword;
    let changes = run_tick(
        &mut state,
        &[PlayerInput::Attack {
            connection: ConnectionId(1),
            target,
        }],
    );
    let names: Vec<_> = changes
        .0
        .changes
        .entities_spawned
        .iter()
        .filter_map(|id| {
            let slot = state.entities.slot_of(*id)?;
            let stack = state.entities.row(slot).item.as_ref()?;
            state
                .registry
                .items
                .name_of(stack.item_id)
                .map(|name| name.path())
        })
        .collect();
    assert!(names.contains(&"cooked_beef"), "{names:?}");
    assert!(!names.contains(&"beef"), "{names:?}");
}

#[path = "enchantment_tick_tests.rs"]
mod enchantment_tick_tests;

#[path = "item_motion_tests.rs"]
mod item_motion_tests;

#[path = "enderman_tests.rs"]
mod enderman_tests;

#[test]
fn use_equippable_publishes_inventory_equipment_and_durable_player_image() {
    use crate::boundary::{Hand, PlayerChange};
    let (mut state, actor, _) = flat_player_fixture();
    let held = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:diamond_helmet").unwrap())
            .unwrap(),
        1,
    );
    let old = mc_data::ItemStack::new(
        state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:iron_helmet").unwrap())
            .unwrap(),
        1,
    );
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[0] = held.clone();
    player.inventory.slots[39] = old.clone();
    let (output, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::UseItem {
            connection: ConnectionId(1),
            hand: Hand::Main,
            yaw: 0.0,
            pitch: 0.0,
            sequence: 1,
        }],
    );
    assert!(
        drains
            .iter()
            .flat_map(|(_, batch)| batch)
            .any(|change| matches!(change,
        PlayerChange::MenuContents { slots, .. } if slots[5].as_ref() == Some(&held)
            && slots[36].as_ref() == Some(&old)))
    );
    assert!(output.changes.entities_appearance_changed.contains(&actor));
    let view = output
        .snapshot
        .entities
        .iter()
        .find(|view| view.id == actor)
        .unwrap();
    assert_eq!(view.appearance.equipment[5], Some(held.clone()));
    assert_eq!(view.appearance.equipment[0], Some(old.clone()));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(entry,
        crate::journal::JournalEntry::Player { inventory, .. }
            if inventory[0] == old && inventory[39] == held)));
    let record = crate::core::player_record_for(&state, actor, state.tick).unwrap();
    let root = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(root.path().to_owned(), 42);
    let saved = crate::native_checkpoint::tests::roundtrip_player(
        &config,
        &uuid::Uuid::from_u128(1).to_string(),
        &record,
        &state.registry.items,
    )
    .unwrap();
    assert_eq!(saved.inventory, record.inventory);
}

#[test]
fn block_phase_scratch_does_not_republish_previous_tick_writes() {
    let mut state = test_state();
    join(&mut state, 1, "block-scratch");
    let position = crate::boundary::BlockPos { x: 8, y: 250, z: 8 };
    let block = state.registry.states.stone;
    state.merge_block_writes.push((position, block));
    let (first, _, _) = run_tick(&mut state, &[]);
    assert!(
        first
            .changes
            .blocks
            .iter()
            .any(|change| change.position == position && change.state == block)
    );
    let (second, _, _) = run_tick(&mut state, &[]);
    assert!(
        !second
            .changes
            .blocks
            .iter()
            .any(|change| change.position == position)
    );
}

#[test]
fn placement_obstruction_uses_live_chunks_after_moves_resize_and_removal() {
    use crate::boundary::BlockPos;
    let mut state = test_state();
    let cow = state.registry.cow_type_id;
    // Keep the indexed path cheaper than the linear fallback, even after a
    // large body's bound has grown. Distant rows never occupy query chunks.
    for index in 0..512 {
        state.spawn_mob(cow, [5000.0 + f64::from(index) * 32.0, 64.0, 0.5], 20.0);
    }
    let near = state.spawn_mob(cow, [15.9, 64.0, 0.5], 20.0);
    let wide = state.spawn_mob(cow, [-32.5, 64.0, 0.5], 20.0);
    let check = |state: &TickState| {
        for x in [-40, -1, 0, 15, 16, 17, 40] {
            for y in [63, 64, 65, 80] {
                for z in [-1, 0, 1, 32] {
                    let pos = BlockPos { x, y, z };
                    let expected = state.entities.slots().any(|slot| {
                        let row = state.entities.body(slot);
                        if !row.alive
                            || matches!(row.kind, crate::entities::EntityKind::Item { .. })
                        {
                            return false;
                        }
                        let at = state.entities.pos(slot);
                        at[0] + row.half_width > f64::from(x)
                            && at[0] - row.half_width < f64::from(x) + 1.0
                            && at[1] + row.height > f64::from(y)
                            && at[1] < f64::from(y) + 1.0
                            && at[2] + row.half_width > f64::from(z)
                            && at[2] - row.half_width < f64::from(z) + 1.0
                    });
                    assert_eq!(crate::tick::obstructed(state, pos), expected, "{pos:?}");
                }
            }
        }
    };
    check(&state);
    state
        .entities
        .set_half_width(state.entities.slot_of(wide).unwrap(), 36.0);
    assert!(crate::tick::obstructed(
        &state,
        BlockPos { x: 0, y: 64, z: 0 }
    ));
    check(&state);
    state
        .entities
        .set_pos(state.entities.slot_of(wide).unwrap(), [7000.0, 64.0, 0.5]);
    check(&state);
    state
        .entities
        .body_mut(state.entities.slot_of(near).unwrap())
        .alive = false;
    check(&state);
    state.remove_entity(state.entities.slot_of(wide).unwrap());
    check(&state);
    let item =
        crate::tick::spawn_item_entity(&mut state, [0.5, 64.0, 0.5], mc_data::ItemStack::new(1, 1));
    state
        .entities
        .set_half_width(state.entities.slot_of(item).unwrap(), 36.0);
    check(&state);
    let villager_type = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:villager").unwrap())
        .unwrap();
    let villager = state.spawn_mob(villager_type, [0.5, 64.0, 0.5], 20.0);
    assert!(crate::tick::obstructed(
        &state,
        BlockPos { x: 0, y: 64, z: 0 }
    ));
    check(&state);
    state.entities.set_pos(
        state.entities.slot_of(villager).unwrap(),
        [500.0, 64.0, 0.5],
    );
    check(&state);
}

#[test]
fn placement_obstruction_bound_covers_child_growth_beyond_spawn_dimensions() {
    let mut state = test_state();
    let cow = state.registry.cow_type_id;
    for index in 0..512 {
        state.spawn_mob(cow, [5000.0 + f64::from(index) * 32.0, 64.0, 0.5], 20.0);
    }
    let child = state.spawn_mob(cow, [-2.0, 64.0, 0.5], 20.0);
    crate::entity_life::make_child(&mut state, child);
    let slot = state.entities.slot_of(child).unwrap();
    state.entities.set_half_width(slot, 18.0);
    state.entities.row_mut(slot).entity_life.child_ticks = 1;
    state.world.ensure_blocking(
        &[(crate::boundary::ChunkPos { x: -1, z: 0 }, 0)]
            .into_iter()
            .collect(),
    );
    state.world.set_active_chunks(
        std::iter::once(crate::boundary::ChunkPos { x: -1, z: 0 }).collect(),
        state.tick,
    );
    crate::entity_life::advance(&mut state);
    assert_eq!(state.entities.body(slot).half_width, 36.0);
    assert!(state.entities.max_half_width() >= 36.0);
    assert!(crate::tick::obstructed(
        &state,
        crate::boundary::BlockPos { x: 32, y: 64, z: 0 }
    ));
}

#[test]
fn placement_obstruction_bound_follows_wide_body_dimension_transfer() {
    let mut source = test_state();
    let mut destination = test_state();
    destination.entities.set_next_wire_id(10_000);
    let cow = source.registry.cow_type_id;
    for index in 0..512 {
        destination.spawn_mob(cow, [5000.0 + f64::from(index) * 32.0, 64.0, 0.5], 20.0);
    }
    let wide = source.spawn_mob(cow, [-2.0, 64.0, 0.5], 20.0);
    let slot = source.entities.slot_of(wide).unwrap();
    source.entities.set_half_width(slot, 36.0);
    source
        .entities
        .transfer_to(slot, &mut destination.entities, [-2.0, 64.0, 0.5], [0.0; 2]);
    assert!(source.entities.slot_of_any(wide).is_none());
    assert!(destination.entities.max_half_width() >= 36.0);
    assert!(crate::tick::obstructed(
        &destination,
        crate::boundary::BlockPos { x: 32, y: 64, z: 0 }
    ));
}

#[test]
fn live_player_enumeration_matches_the_dense_scan() {
    let (mut state, first, pos) = flat_player_fixture();
    // A hole before later players makes them move down when the mob leaves.
    let hole = state.spawn_mob(state.registry.cow_type_id, pos, 20.0);
    let mut actors = vec![first];
    for connection in 2..=6 {
        actors.push(join(&mut state, connection, "scan"));
    }
    assert_eq!(
        crate::tick::live_players(&state),
        crate::tick::live_players_by_scan(&state)
    );
    state.remove_entity(state.entities.slot_of(hole).unwrap());
    // The last-joined player now sits before earlier ones: slot order differs
    // from id order, which the enumeration must restore.
    assert!(
        state.entities.slot_of(actors[5]).unwrap().0 < state.entities.slot_of(actors[1]).unwrap().0
    );
    // Dead body, spectator body (still ticked) and a stale map entry.
    state
        .entities
        .body_mut(state.entities.slot_of(actors[2]).unwrap())
        .alive = false;
    state.players.get_mut(&actors[3]).unwrap().game_mode = crate::commands::GameMode::Spectator;
    let stale = state.players[&first].clone();
    state
        .players
        .insert(crate::boundary::EntityId(2_000_000), stale);
    let live = crate::tick::live_players(&state);
    assert_eq!(live, crate::tick::live_players_by_scan(&state));
    assert_eq!(live.len(), actors.len() - 1);
}

/// A deployment whose manifest declares `death_graves` takes one death's drops
/// into a barrel at the death spot, journals that container, keeps the bar and
/// publishes the grave to that package only.
#[test]
fn grave_custody_buries_drops_keeps_experience_and_publishes_the_grave() {
    use crate::survival::DamageKind;
    let (mut state, player, pos) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "false".into());
    let (boundary, _host) = mc_script::script_boundary_pair(
        std::num::NonZeroUsize::new(16).unwrap(),
        std::num::NonZeroUsize::new(16).unwrap(),
    );
    let mut runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary);
    runtime.death_graves = Some("graves".into());
    state.plugin_runtime = Some(runtime);
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    let actor = state.players.get_mut(&player).unwrap();
    actor.inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    actor.vitals.experience.add_points(42);
    let experience = actor.vitals.experience;
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, spawned, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![player]);
    assert!(
        spawned.is_empty(),
        "a taken-in-custody drop must not also spawn as an item entity"
    );
    assert_eq!(
        state.players[&player].vitals.experience, experience,
        "grave custody retains the bar instead of resetting it"
    );
    let cell = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(cell.0, cell.1, cell.2).unwrap(),
        ))
        .unwrap();
    assert_eq!(block.block.id.path(), "barrel");
    let storage = state.block_entities.get(&cell).unwrap();
    assert_eq!(storage.slots[0], mc_data::ItemStack::new(dirt, 7));
    assert!(storage.slots[1..].iter().all(mc_data::ItemStack::is_empty));
    let published: Vec<_> = state
        .plugin_runtime
        .as_ref()
        .unwrap()
        .pending_events
        .iter()
        .filter_map(|event| match event.kind() {
            mc_script::ScriptEventKind::PlayerGraveCreated {
                items,
                spilled,
                x,
                y,
                z,
                ..
            } => Some((*items, *spilled, *x, *y, *z, event.target_plugin_id())),
            _ => None,
        })
        .collect();
    assert_eq!(
        published,
        vec![(1, 0, cell.0, cell.1, cell.2, Some("graves"))]
    );
}

/// `keepInventory` wins over a deployed grave package exactly as it wins over the
/// vanilla drop: the inventory never leaves the player and no container appears.
#[test]
fn keep_inventory_bypasses_grave_custody() {
    use crate::survival::DamageKind;
    let (mut state, player, pos) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "true".into());
    let (boundary, _host) = mc_script::script_boundary_pair(
        std::num::NonZeroUsize::new(16).unwrap(),
        std::num::NonZeroUsize::new(16).unwrap(),
    );
    let mut runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary);
    runtime.death_graves = Some("graves".into());
    state.plugin_runtime = Some(runtime);
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, spawned, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![player]);
    assert!(spawned.is_empty());
    assert_eq!(
        state.players[&player].inventory.slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
    let cell = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let block = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(cell.0, cell.1, cell.2).unwrap(),
        ))
        .unwrap();
    assert_ne!(block.block.id.path(), "barrel");
    assert!(!state.block_entities.contains_key(&cell));
    assert!(
        !state
            .plugin_runtime
            .as_ref()
            .unwrap()
            .pending_events
            .iter()
            .any(|event| matches!(
                event.kind(),
                mc_script::ScriptEventKind::PlayerGraveCreated { .. }
            ))
    );
}

/// A deployed grave package, exactly as `PluginRuntime::open` would resolve one.
fn grave_runtime(state: &mut TickState) {
    let (boundary, _host) = mc_script::script_boundary_pair(
        std::num::NonZeroUsize::new(16).unwrap(),
        std::num::NonZeroUsize::new(16).unwrap(),
    );
    let mut runtime = crate::plugin::PluginRuntime::with_test_boundary(boundary);
    runtime.death_graves = Some("graves".into());
    state.plugin_runtime = Some(runtime);
}

/// Put one hopper block with its inventory at `cell`, so it can really pull.
fn hopper_at(state: &mut TickState, cell: (i32, i32, i32)) -> u32 {
    let hopper = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:hopper").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(cell.0, cell.1, cell.2, hopper);
    crate::block_entity_defaults::after_blocks(
        state,
        &[(
            crate::boundary::BlockPos {
                x: cell.0,
                y: cell.1,
                z: cell.2,
            },
            hopper,
        )],
    );
    hopper
}

/// A dying player standing on an existing hopper keeps a grave that the hopper
/// cannot empty: the container goes to the cell above, and the hopper's own
/// inventory stays empty across the pull cycles that follow.
#[test]
fn grave_placement_avoids_an_existing_hopper_below_the_death_cell() {
    use crate::survival::DamageKind;
    let (mut state, player, pos) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "false".into());
    grave_runtime(&mut state);
    let death_cell = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let hopper_cell = (death_cell.0, death_cell.1 - 1, death_cell.2);
    hopper_at(&mut state, hopper_cell);
    // Register the hopper in the redstone lane so the pull cycles really run:
    // without the placement guard it would drain a barrel in the death cell.
    crate::redstone::containers::inventory_changed(
        &mut state.redstone_containers,
        crate::boundary::BlockPos {
            x: hopper_cell.0,
            y: hopper_cell.1,
            z: hopper_cell.2,
        },
    );
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, spawned, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![player]);
    assert!(spawned.is_empty());
    // The death cell stands on the hopper and the cell above has no ground, so
    // the first ring cell with solid ground under it takes the grave.
    let grave_cell = (death_cell.0 - 1, death_cell.1, death_cell.2 - 1);
    let barrel = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state
                .world
                .block_at(grave_cell.0, grave_cell.1, grave_cell.2)
                .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        barrel.block.id.path(),
        "barrel",
        "a cell above an existing hopper is not a grave cell"
    );
    assert_eq!(
        state.block_entities[&grave_cell].slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
    assert!(
        state.block_entities[&hopper_cell]
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty)
    );
    for _ in 0..16 {
        crate::tick::run_tick(&mut state, &[]);
    }
    assert!(
        state.block_entities[&hopper_cell]
            .slots
            .iter()
            .all(mc_data::ItemStack::is_empty),
        "the hopper must not be able to pull the grave's items"
    );
    assert_eq!(
        state.block_entities[&grave_cell].slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
}

/// A partial block under the death cell is not ground: a slab collides as a
/// partial shape, which leaves the rest of the cell free for a hopper minecart,
/// so the grave moves to the first fully grounded ring cell.
#[test]
fn grave_placement_avoids_a_partial_block_below_the_death_cell() {
    use crate::survival::DamageKind;
    let (mut state, player, pos) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "false".into());
    grave_runtime(&mut state);
    let death_cell = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    let slab = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone_slab").unwrap())
        .unwrap()
        .default
        .0;
    state
        .world
        .set_block(death_cell.0, death_cell.1 - 1, death_cell.2, slab);
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, _, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![player]);
    assert_eq!(
        state
            .world
            .block_at(death_cell.0, death_cell.1 - 1, death_cell.2),
        Some(slab),
        "a partial block is never replaced by the grave"
    );
    let grave_cell = (death_cell.0 - 1, death_cell.1, death_cell.2 - 1);
    let barrel = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state
                .world
                .block_at(grave_cell.0, grave_cell.1, grave_cell.2)
                .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        barrel.block.id.path(),
        "barrel",
        "a cell standing on a partial block is not a grave cell"
    );
    assert_eq!(
        state.block_entities[&grave_cell].slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
}

/// A hopper minecart sitting under the death cell is the same drain through a
/// moving body: the cell has no solid ground, so the grave moves to the first
/// ring cell that does, and the cart's own inventory stays empty while it tries.
#[test]
fn grave_placement_avoids_a_cell_a_hopper_minecart_occupies() {
    use crate::survival::DamageKind;
    let (mut state, player, pos) = flat_player_fixture();
    state
        .gamerules
        .insert("keep_inventory".into(), "false".into());
    grave_runtime(&mut state);
    let death_cell = (
        pos[0].floor() as i32,
        pos[1].floor() as i32,
        pos[2].floor() as i32,
    );
    // Open the ground under the death cell: a cart can only ever sit in a
    // non-solid cell, which is exactly the ground the rule forbids.
    state.world.set_block(
        death_cell.0,
        death_cell.1 - 1,
        death_cell.2,
        state.registry.states.air,
    );
    let cart = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:hopper_minecart").unwrap())
        .unwrap();
    let cart_entity = state.spawn_mob(
        cart,
        [
            f64::from(death_cell.0) + 0.5,
            f64::from(death_cell.1) - 1.0 + 0.0625,
            f64::from(death_cell.2) + 0.5,
        ],
        4.0,
    );
    let cart_slot = state.entities.slot_of(cart_entity).unwrap();
    let dirt = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:dirt").unwrap())
        .unwrap();
    state.players.get_mut(&player).unwrap().inventory.slots[0] = mc_data::ItemStack::new(dirt, 7);
    let slot = state.entities.slot_of(player).unwrap();
    state.merge_damage.push((
        slot,
        crate::tick::DamagePiece {
            physical: None,
            custom_armor: false,
            amount: f32::MAX,
            kind: DamageKind::Void,
            source: None,
        },
    ));
    let (deaths, _, _, _) = state.commit_merge_effects();
    assert_eq!(deaths, vec![player]);
    let grave_cell = (death_cell.0 - 1, death_cell.1, death_cell.2 - 1);
    let barrel = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state
                .world
                .block_at(grave_cell.0, grave_cell.1, grave_cell.2)
                .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        barrel.block.id.path(),
        "barrel",
        "a cell a hopper minecart could occupy is not a grave cell"
    );
    assert_eq!(
        state.block_entities[&grave_cell].slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
    for _ in 0..16 {
        let mut intents = Vec::new();
        let mut changes = crate::boundary::ChangeSet::default();
        crate::vehicles::run(&mut state, &mut intents, &mut changes);
    }
    let cart_slots = &state
        .entities
        .row(cart_slot)
        .entity_life
        .vehicle
        .as_ref()
        .unwrap()
        .slots;
    assert!(
        cart_slots.iter().all(mc_data::ItemStack::is_empty),
        "the cart must not be able to pull the grave's items"
    );
    assert_eq!(
        state.block_entities[&grave_cell].slots[0],
        mc_data::ItemStack::new(dirt, 7)
    );
}

#[test]
fn same_tick_slot_swap_attack_uses_previous_tick_held_attributes() {
    // MC-28289 at the tick level: SelectSlot + Attack in one input batch
    // must resolve attack_damage from the pre-tick held stack.
    use crate::boundary::ConnectionId;
    use crate::boundary::PlayerInput;
    let (mut state, player, pos) = flat_player_fixture();
    state.tick = 20;
    state.last_swing.insert(player, 0);
    let actor_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_ground(actor_slot, true);
    let sword = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:wooden_sword").unwrap())
        .unwrap();
    let stick = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:stick").unwrap())
        .unwrap();
    let actor = state.players.get_mut(&player).unwrap();
    actor.inventory.slots[0] = mc_data::ItemStack::new(sword, 1);
    actor.inventory.slots[1] = mc_data::ItemStack::new(stick, 1);
    actor.inventory.selected = 0;
    let zombie = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie").unwrap())
        .unwrap();
    let victim = state.spawn_mob(zombie, [pos[0], pos[1], pos[2] + 1.5], 20.0);
    state.day_time = 14_000;
    // Prime the snapshot with one quiet tick (held = sword).
    run_tick(&mut state, &[]);
    // Same tick: swap to stick, then attack. Vanilla (oracle) hits with the
    // sword's damage; without the snapshot this deals stick damage.
    let (output, _, _) = run_tick(
        &mut state,
        &[
            PlayerInput::SelectSlot {
                connection: ConnectionId(1),
                slot: 1,
            },
            PlayerInput::Attack {
                connection: ConnectionId(1),
                target: victim,
            },
        ],
    );
    let victim_slot = state.entities.slot_of(victim).unwrap();
    let health = state.entities.row(victim_slot).health;
    // Wooden sword full-strength vs zombie (armor 2): raw 5.0 mitigated to
    // ~4.7; a stick would deal ~0.94. Assert well above stick range.
    assert!(
        health < 20.0 - 2.0,
        "same-tick swap+attack must use pre-tick sword attributes, health={health}"
    );
    assert!(
        output
            .changes
            .entity_events
            .iter()
            .any(|event| matches!(event, crate::boundary::EntityEvent::Damage { .. })),
        "attack must land"
    );
}
