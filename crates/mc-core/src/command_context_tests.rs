use super::*;
use crate::boundary::BlockPos;

fn fixture() -> (TickState, EntityId) {
    let (mut state, entity, _) = crate::hostile_tests::setup(8.0);
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [1.75, 60.0, 2.25]);
    state.entities.set_rot(slot, 0.0, 0.0);
    state.day_time = 0;
    (state, entity)
}
fn command(state: &mut TickState, entity: EntityId, text: &str) {
    let context = Context::source(state, crate::command_console::CONNECTION, entity, 2);
    run(state, text, context, &mut ChangeSet::default());
}
fn diamond(state: &TickState, position: [i32; 3]) -> bool {
    let [x, y, z] = position;
    state
        .world
        .block_at(x, y, z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == "diamond_block")
}

#[test]
fn command_parity_configured_fork_budget_rejects_before_execution() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "gamerule minecraft:max_command_forks 0");
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 0);
}

#[test]
fn command_parity_fork_limit_is_strict_and_sequence_costs_modifier_stages() {
    let (mut state, entity) = fixture();
    command(&mut state, entity, "gamerule minecraft:max_command_forks 1");
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 0);
    command(&mut state, entity, "gamerule minecraft:max_command_forks 2");
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    command(&mut state, entity, "execute as @s run time add 1");
    assert_eq!(state.day_time, 1);
    command(&mut state, entity, "execute as @s at @s run time add 1");
    assert_eq!(state.day_time, 1);
    command(&mut state, entity, "time add 1");
    assert_eq!(state.day_time, 2);
}

#[test]
fn command_parity_zero_sequence_budget_still_admits_one_root_command() {
    // COMMAND-ZERO-BUDGET-LOCKOUT: a zero gamerule clamps the root quota to
    // one so ordinary and console administration can restore a positive
    // limit instead of rejecting every native root command.
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 0",
    );
    command(&mut state, entity, "time add 5");
    assert_eq!(state.day_time, 5);
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 2",
    );
    command(&mut state, entity, "time add 1");
    assert_eq!(state.day_time, 6);
}

#[test]
fn positioned_align_rotated_and_anchor_affect_native_local_coordinates() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 4.8 60 4.9 align xz rotated 0 0 run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [4, 60, 5]));
    command(
        &mut state,
        entity,
        "execute positioned 6 60 6 anchored eyes run setblock ^ ^ ^ minecraft:diamond_block",
    );
    assert!(diamond(&state, [6, 61, 6]));
    command(
        &mut state,
        entity,
        "execute positioned 7 60 7 anchored eyes run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(
        diamond(&state, [7, 60, 7]),
        "anchor applies to local, not relative coordinates"
    );
    command(
        &mut state,
        entity,
        "execute positioned 8 60 8 anchored eyes anchored feet run setblock ^ ^ ^ minecraft:diamond_block",
    );
    assert!(diamond(&state, [8, 60, 8]));
    assert!(state.command_context.current.is_none());
}
#[test]
fn as_changes_executor_at_changes_pose_and_positioned_as_keeps_rotation() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 as @s run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(&state, [10, 60, 10]), "as must not move the source");
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 at @s run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(&state, [1, 60, 2]));
    command(
        &mut state,
        entity,
        "execute rotated 90 0 positioned as @s run setblock ^ ^ ^2 minecraft:diamond_block",
    );
    assert!(diamond(&state, [-1, 60, 2]));
    command(
        &mut state,
        entity,
        "execute positioned 10 60 10 rotated as @s run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [10, 60, 11]));
}
#[test]
fn facing_coordinates_and_entity_anchor_rotate_native_source() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 4 60 4 facing 4 60 8 run setblock ^ ^ ^2 minecraft:diamond_block",
    );
    assert!(diamond(&state, [4, 60, 6]));
    command(
        &mut state,
        entity,
        "execute positioned 1.75 60 0.25 facing entity @s feet run setblock ^ ^ ^1 minecraft:diamond_block",
    );
    assert!(diamond(&state, [1, 60, 1]));
}
#[test]
fn if_unless_entity_block_and_score_filter_without_mutating_source() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "setblock 4 60 4 minecraft:diamond_block",
    );
    command(
        &mut state,
        entity,
        "execute if entity @s if block 4 60 4 minecraft:diamond_block run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute unless entity @e[type=minecraft:zombie] unless block 4 60 4 minecraft:stone run time add 2",
    );
    command(&mut state, entity, "scoreboard objectives add test dummy");
    command(&mut state, entity, "scoreboard players set left test 7");
    command(&mut state, entity, "scoreboard players set right test 9");
    command(
        &mut state,
        entity,
        "execute if score left test matches 5..8 if score left test < right test run time add 4",
    );
    command(
        &mut state,
        entity,
        "execute unless score left test >= right test run time add 8",
    );
    command(
        &mut state,
        entity,
        "execute if score absent test matches 0 run time add 100",
    );
    command(
        &mut state,
        entity,
        "execute if block 4 60 4 minecraft:stone run time add 100",
    );
    assert_eq!(state.day_time, 15);
}
#[test]
fn selector_origin_uses_positioned_context() {
    let (mut state, entity) = fixture();
    command(
        &mut state,
        entity,
        "execute positioned 1.75 60 2.25 if entity @e[distance=..0.1] run time add 1",
    );
    command(
        &mut state,
        entity,
        "execute positioned 12 60 12 if entity @e[distance=..0.1] run time add 100",
    );
    assert_eq!(state.day_time, 1);
}
#[test]
fn in_uses_shared_world_authority_without_moving_executor_and_commits_blocks() {
    let (mut state, entity) = fixture();
    let (mut nether, _) = fixture();
    nether.dimension = Dimension::Nether;
    state
        .other_dimensions
        .insert(Dimension::Nether, Box::new(nether));
    state.command_context.shared = true;
    command(
        &mut state,
        entity,
        "execute positioned 8 60 8 in minecraft:the_nether run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(diamond(
        &state.other_dimensions[&Dimension::Nether],
        [1, 60, 1]
    ));
    assert_eq!(state.dimension, Dimension::Overworld);
    assert!(state.entities.slot_of(entity).is_some());
    let changes = &state.command_context.changes[&Dimension::Nether];
    assert!(
        changes
            .blocks
            .iter()
            .any(|change| change.position == BlockPos { x: 1, y: 60, z: 1 })
    );
    assert!(
        state.other_dimensions[&Dimension::Nether]
            .merge_block_writes
            .is_empty()
    );
    state.command_context.shared = false;
    command(
        &mut state,
        entity,
        "execute positioned 16 60 16 in minecraft:the_nether run setblock ~ ~ ~ minecraft:diamond_block",
    );
    assert!(
        !diamond(&state.other_dimensions[&Dimension::Nether], [2, 60, 2]),
        "no bypass outside shared boundary"
    );
}
#[test]
fn nested_execute_budget_and_permissions_fail_closed() {
    let (mut state, entity) = fixture();
    let mut context = Context::source(&state, crate::command_console::CONNECTION, entity, 0);
    run(
        &mut state,
        "execute run time add 100",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.day_time, 0);
    context.permission = 2;
    // Zero clamps the root quota to one command: a single chain leaf still
    // executes (no lockout), while a modifier stage plus two leaves exceeds
    // the single quota and fails closed (see the zero-budget test above).
    command(
        &mut state,
        entity,
        "gamerule minecraft:max_command_sequence_length 0",
    );
    run(
        &mut state,
        "execute as @s run execute as @s run time add 100",
        context,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.day_time, 0);
    assert!(state.command_context.current.is_none());
    assert_eq!(state.command_context.depth, 0);
}
