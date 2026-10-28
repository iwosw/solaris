use super::*;

fn above(base: BlockPos, dy: i32) -> BlockPos {
    BlockPos {
        y: base.y + dy,
        ..base
    }
}

fn cell(state: &TickState, pos: BlockPos) -> u32 {
    state.world.block_at(pos.x, pos.y, pos.z).expect("resident")
}

fn tank(state: &mut TickState, base: BlockPos, height: i32) {
    let stone = state.registry.states.stone;
    for dy in 1..=height {
        for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            place(
                state,
                BlockPos {
                    x: base.x + dx,
                    y: base.y + dy,
                    z: base.z + dz,
                },
                stone,
            );
        }
    }
}

#[test]
fn bubble_columns_create_switch_and_remove_through_source_water() {
    let (mut state, _, centre) = setup();
    let base = above(centre, -1);
    tank(&mut state, base, 4);
    let water = state.registry.states.water_levels[0];
    let soul = block(&state, "soul_sand", &[]);
    let magma = block(&state, "magma_block", &[]);
    let up = block(&state, "bubble_column", &[("drag", "false")]);
    let down = block(&state, "bubble_column", &[("drag", "true")]);
    for dy in 1..=4 {
        place(&mut state, above(base, dy), water);
    }
    place(&mut state, base, soul);
    run(&mut state, 100);
    for dy in 1..=4 {
        assert_eq!(cell(&state, above(base, dy)), up, "up at {dy}");
    }
    place(&mut state, base, magma);
    run(&mut state, 100);
    for dy in 1..=4 {
        assert_eq!(cell(&state, above(base, dy)), down, "down at {dy}");
    }
    let stone = state.registry.states.stone;
    place(&mut state, base, stone);
    run(&mut state, 100);
    for dy in 1..=4 {
        assert_eq!(cell(&state, above(base, dy)), water, "removed at {dy}");
    }
}

#[test]
fn bubble_columns_disconnect_above_a_replaced_middle_block() {
    let (mut state, _, centre) = setup();
    let base = above(centre, -1);
    tank(&mut state, base, 4);
    let water = state.registry.states.water_levels[0];
    let soul = block(&state, "soul_sand", &[]);
    let up = block(&state, "bubble_column", &[("drag", "false")]);
    for dy in 1..=4 {
        place(&mut state, above(base, dy), water);
    }
    place(&mut state, base, soul);
    run(&mut state, 100);
    let stone = state.registry.states.stone;
    place(&mut state, above(base, 2), stone);
    run(&mut state, 100);
    assert_eq!(cell(&state, above(base, 1)), up);
    assert_eq!(cell(&state, above(base, 2)), stone);
    assert_eq!(cell(&state, above(base, 3)), water);
    assert_eq!(cell(&state, above(base, 4)), water);
}

#[test]
fn bubble_columns_preserve_supported_manual_cells_and_stop_at_boundaries() {
    let (mut state, _, centre) = setup();
    let base = above(centre, -1);
    tank(&mut state, base, 5);
    let soul = block(&state, "soul_sand", &[]);
    let up = block(&state, "bubble_column", &[("drag", "false")]);
    let water = state.registry.states.water_levels[0];
    let flowing = state.registry.states.water_levels[1];
    let stone = state.registry.states.stone;
    place(&mut state, base, soul);
    place(&mut state, above(base, 1), up);
    place(&mut state, above(base, 2), water);
    place(&mut state, above(base, 3), stone);
    place(&mut state, above(base, 4), water);
    run(&mut state, 100);
    assert_eq!(cell(&state, above(base, 1)), up);
    assert_eq!(cell(&state, above(base, 2)), up);
    assert_eq!(cell(&state, above(base, 3)), stone);
    assert_eq!(cell(&state, above(base, 4)), water);
    place(&mut state, above(base, 2), stone);
    run(&mut state, 100);
    assert_eq!(cell(&state, above(base, 1)), up);
    assert_eq!(cell(&state, above(base, 2)), stone);
    // A directly scheduled flowing cell must not become a bubble column.
    place(&mut state, above(base, 2), flowing);
    run(&mut state, 5);
    assert_ne!(cell(&state, above(base, 2)), up);
}
