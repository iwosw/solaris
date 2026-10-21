use super::tests::{attempt, complete, fixture, fund, start};
use super::*;

fn first_cell(state: &TickState, blueprint: &str, path: &str) -> ([i32; 3], u32) {
    let level = blueprints::get(&state.registry, blueprint)
        .unwrap()
        .level(1)
        .unwrap()
        .rotated(Rotation::from_quarter_turns(0));
    let (offset, block) = level
        .blocks
        .iter()
        .find(|(_, block)| state.registry.blocks.by_id(*block).unwrap().block.id.path() == path)
        .unwrap();
    (level.world([3, 200, 3], *offset), block.0)
}

#[test]
fn farmland_drying_and_wheat_growth_during_native_work_do_not_pause_construction() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "farm", 1, None);
    fund(&mut state, "farm", 1);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let (soil, soil_state) = first_cell(&state, "farm", "farmland");
    let (crop, crop_state) = first_cell(&state, "farm", "wheat");
    for _ in 0..4096 {
        if state.world.block_at(soil[0], soil[1], soil[2]) == Some(soil_state)
            && state.world.block_at(crop[0], crop[1], crop[2]) == Some(crop_state)
        {
            break;
        }
        attempt(&mut state, id, slot);
    }
    assert!(
        state.settlements.get(id).unwrap().project.is_some(),
        "evolution occurs mid-build"
    );
    assert_eq!(
        state.world.block_at(crop[0], crop[1], crop[2]),
        Some(crop_state)
    );
    let dry =
        crate::farming::with_prop(&state.registry.blocks, soil_state, "moisture", "0").unwrap();
    let grown = crate::farming::with_prop(&state.registry.blocks, crop_state, "age", "1").unwrap();
    state.world.set_block(soil[0], soil[1], soil[2], dry);
    state.world.set_block(crop[0], crop[1], crop[2], grown);
    let before = executor::status(&state, id, &project.structure).unwrap();
    attempt(&mut state, id, slot);
    attempt(&mut state, id, slot);
    let after = executor::status(&state, id, &project.structure).unwrap();
    assert_ne!(after.state, ScriptStructureState::Paused);
    assert!(
        after.revision > before.revision,
        "canonical work advances after state evolution"
    );
    complete(&mut state, id, slot);
    crate::plugin_structures::validate(&state).unwrap();
}

#[test]
fn foreign_plank_replacement_pauses_without_debit_then_next_admission_resumes_and_completes() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let (plank, plank_state) = first_cell(&state, "house", "oak_planks");
    for _ in 0..4096 {
        if state.world.block_at(plank[0], plank[1], plank[2]) == Some(plank_state) {
            break;
        }
        attempt(&mut state, id, slot);
    }
    assert!(state.settlements.get(id).unwrap().project.is_some());
    let before = executor::status(&state, id, &project.structure).unwrap();
    let writes = state.plugin_block_writes.len();
    state
        .world
        .set_block(plank[0], plank[1], plank[2], state.registry.states.stone);
    // The executor still refuses to place over a changed footprint.
    let mut paused = before.clone();
    for _ in 0..8 {
        attempt(&mut state, id, slot);
        paused = executor::status(&state, id, &project.structure).unwrap();
        if paused.state == ScriptStructureState::Paused {
            break;
        }
    }
    assert_eq!(paused.state, ScriptStructureState::Paused);
    assert_eq!(paused.pause_reason.as_deref(), Some("footprint_changed"));
    assert_eq!(paused.completed_work_units, before.completed_work_units);
    assert_eq!(paused.consumed, before.consumed);
    assert_eq!(state.plugin_block_writes.len(), writes);
    // The next construction admission re-snapshots the footprint and resumes.
    attempt(&mut state, id, slot);
    let resumed = executor::status(&state, id, &project.structure).unwrap();
    assert_ne!(resumed.state, ScriptStructureState::Paused);
    assert_eq!(resumed.pause_reason, None);
    assert_eq!(resumed.completed_work_units, before.completed_work_units);
    complete(&mut state, id, slot);
    assert_eq!(
        state.world.block_at(plank[0], plank[1], plank[2]),
        Some(state.registry.states.stone),
        "recovery never rewrites cells that were already placed"
    );
    crate::plugin_structures::validate(&state).unwrap();
}
