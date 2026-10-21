use super::tests::{fixture, fund, start};
use super::*;

#[test]
fn one_builder_advance_seals_block_debit_and_cursor_before_closed_writer_replay() {
    let (mut state, id, slot) = fixture();
    start(&mut state, id, "house", 1, None);
    fund(&mut state, "house", 1);
    let project = state.settlements.get(id).unwrap().project.clone().unwrap();
    let next = executor::next_cell(&state, id, &project.structure)
        .unwrap()
        .unwrap();
    let next_item = executor::next_material(&state, id, &project.structure)
        .unwrap()
        .unwrap();
    assert_eq!(
        next_item.1, 1,
        "first house cell is a single real floor item"
    );
    let before_stock: i32 = state.block_entities[&(1, 201, 1)]
        .slots
        .iter()
        .filter(|stack| stack.item_id == next_item.0)
        .map(|stack| stack.count)
        .sum();
    crate::villager::building::tick(&mut state, id); // hauling only; no block advance
    assert!(state.plugin_block_writes.is_empty());
    let hauled: i32 = state
        .entities
        .row(slot)
        .villager
        .as_ref()
        .unwrap()
        .inventory
        .iter()
        .flatten()
        .filter(|stack| stack.item_id == next_item.0)
        .map(|stack| stack.count)
        .sum();
    assert!(
        hauled > 1,
        "a real material batch enters carry before placement"
    );
    assert_eq!(
        executor::status(&state, id, &project.structure)
            .unwrap()
            .completed_work_units,
        0
    );
    let builder_uuid = state.entities.row(slot).uuid.to_string();

    // Exactly one real authoritative tick. No follow-up tick may drain a
    // stranded placement or repair the block/cursor/material relationship.
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let committed = executor::status(&state, id, &project.structure).unwrap();
    assert_eq!(committed.completed_work_units, 1);
    assert_eq!(committed.watermark, 1);
    let placed = state.world.block_at(next[0], next[1], next[2]).unwrap();
    assert!(output.changes.blocks.iter().any(|change| [
        change.position.x,
        change.position.y,
        change.position.z
    ] == next
        && change.state == placed));
    assert!(
        state.plugin_block_writes.is_empty(),
        "late canonical writes seal in this tick"
    );
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let directory = tempfile::tempdir().unwrap();
    {
        let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(output.tick, output.tick, &entries).unwrap();
    }
    let batches =
        crate::journal::read_complete_batches_after(&directory.path().join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    assert_eq!(replay.tick, output.tick);
    assert_eq!(
        replay.blocks.get(&(next[0], next[1], next[2])),
        Some(&placed)
    );
    let carried = replay
        .entities
        .values()
        .find(|entity| entity.uuid == builder_uuid)
        .unwrap()
        .villager
        .as_ref()
        .unwrap();
    assert_eq!(
        carried
            .inventory
            .iter()
            .flatten()
            .filter(|stack| stack.item_id == next_item.0)
            .map(|stack| stack.count)
            .sum::<i32>(),
        hauled - 1,
        "only the placed unit is debited; the real remaining batch survives replay"
    );
    assert_eq!(
        replay.block_entities[&(1, 201, 1)]
            .slots
            .iter()
            .filter(|stack| stack.item_id == next_item.0)
            .map(|stack| stack.count)
            .sum::<i32>(),
        before_stock - hauled
    );
    let (mut restored, _, _) = fixture();
    restored.settlements.apply_image(super::super::RecordImage {
        id,
        value: Some(replay.settlements[&id.0].clone()),
    });
    restored.plugins = replay.plugins;
    for ((x, y, z), block) in replay.blocks {
        restored.world.set_block(x, y, z, block);
    }
    let recovered = executor::status(&restored, id, &project.structure).unwrap();
    assert_eq!(
        recovered, committed,
        "only the same sealed block/debit advances the durable cursor"
    );
    crate::plugin_structures::validate(&restored).unwrap();
}
