use super::*;
use crate::boundary::{PlayerChange, PlayerInput};

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let world = crate::world::ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [20, 80, 30],
        mc_world::OVERWORLD_GEOMETRY,
    );
    TickState::new(registry, world, 1, 4096)
}

#[test]
fn console_boundary_runs_operator_commands_without_registering_an_entity() {
    let mut state = fixture();
    let (command, mut completed) = ConsoleCommand::new("time set 1234".into());
    let (_, drains, _) = crate::tick::run_tick(&mut state, &[PlayerInput::ConsoleCommand(command)]);
    assert!(completed.try_recv().is_ok());
    assert!((1234..=1235).contains(&state.day_time));
    assert_eq!(state.entities.slots().count(), 0);
    assert_eq!(state.connections.all_connections().count(), 0);
    assert!(
        drains
            .iter()
            .any(|(id, messages)| *id == CONNECTION && !messages.is_empty())
    );
}

#[test]
fn console_relative_world_coordinates_use_spawn_and_self_targets_fail() {
    let mut state = fixture();
    let mut changes = ChangeSet::default();
    execute(
        &mut state,
        CONNECTION,
        "setworldspawn ~2 ~3 ~4",
        4,
        &mut changes,
    );
    assert_eq!(state.world.world_spawn(), [22, 83, 34]);
    execute(&mut state, CONNECTION, "gamemode creative", 4, &mut changes);
    assert_eq!(state.players.len(), 0);
    assert!(format!("{:?}", state.outbox[&CONNECTION]).contains("argument.entity.notfound.player"));
}

#[test]
fn console_uses_existing_parser_errors_and_player_permission_gate() {
    let mut state = fixture();
    let mut changes = ChangeSet::default();
    execute(&mut state, CONNECTION, "not-a-command", 4, &mut changes);
    assert!(
        state.outbox[&CONNECTION].iter().any(
            |change| matches!(change, PlayerChange::Chat { message, .. } if !message.is_empty())
        )
    );
    assert!(crate::commands::parse("time set 1234", false).is_err());
    assert!(pose(&state, EntityId(999)).is_none());
}

#[test]
fn console_teleport_of_a_player_moves_it_instead_of_halting_the_core() {
    // The console executor (EntityId(-1)) is not an entity; teleport feedback used to
    // `expect` it to still exist and panicked the tick thread.
    let (mut state, player, _) = crate::hostile_tests::setup(8.0);
    let (command, mut completed) = ConsoleCommand::new("tp target 3.5 90 4.5".into());
    let _ = crate::tick::run_tick(&mut state, &[PlayerInput::ConsoleCommand(command)]);
    assert!(completed.try_recv().is_ok());
    let slot = state.entities.slot_of(player).unwrap();
    assert_eq!(state.entities.pos(slot), [3.5, 90.0, 4.5]);
}
