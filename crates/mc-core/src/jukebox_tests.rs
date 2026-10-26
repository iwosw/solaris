use super::*;
use crate::bees::tests::{POS, item, joined, place, state};
use crate::boundary::ChunkPos;
use std::sync::Arc;

#[test]
fn record_insert_eject_emits_exact_song_events_and_comparator() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:jukebox");
    let disc = item(&state, "minecraft:music_disc_cat", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = disc.clone();
    let mut changes = ChangeSet::default();
    assert!(use_on(&mut state, actor, Hand::Main, POS, &mut changes));
    assert!(playing(&state, POS));
    assert_eq!(comparator(&state, POS), Some(2));
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    let expected = mc_data::required_registry_entry_id(
        "jukebox_song",
        &Identifier::parse("minecraft:cat").unwrap(),
    )
    .unwrap() as i32;
    assert!(state.outbox.values().flatten().any(
        |e| matches!(e,PlayerChange::ProjectileLevelEvent{event:1010,data,..}if *data==expected)
    ));
    use_on(&mut state, actor, Hand::Main, POS, &mut changes);
    assert!(!playing(&state, POS));
    assert_eq!(comparator(&state, POS), Some(0));
    assert_eq!(changes.entities_spawned.len(), 1);
    assert!(state.outbox.values().flatten().any(|e| matches!(
        e,
        PlayerChange::ProjectileLevelEvent {
            event: 1011,
            data: 0,
            ..
        }
    )));
    let row = state
        .entities
        .row(state.entities.slot_of(changes.entities_spawned[0]).unwrap());
    assert_eq!(row.item.as_ref().unwrap().full_stack(), disc);
}
#[test]
fn song_end_retains_record_and_checkpoint_preserves_remaining_clock() {
    let dir = std::env::temp_dir().join(format!("jukebox-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let actor = joined(&mut original);
    place(&mut original, POS, "minecraft:jukebox");
    let disc = item(&original, "minecraft:music_disc_11", 1);
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = disc;
    use_on(
        &mut original,
        actor,
        Hand::Main,
        POS,
        &mut ChangeSet::default(),
    );
    original.jukeboxes.get_mut(&key(POS)).unwrap().elapsed = Some(1439);
    checkpoint(&mut original);
    {
        let capture = original.world.capture_dirty(&BTreeMap::new());
        assert!(!capture.write().unwrap().is_empty());
        let reader = capture.store().unwrap().checkpoint_reader().unwrap();
        let generator =
            mc_worldgen::TerrainGenerator::new(42, Arc::clone(&original.registry.blocks));
        let chunk = reader
            .read_chunk(
                0,
                ChunkPos { x: 0, z: 0 },
                &original.registry.blocks,
                &original.registry.items,
                &original.registry.chest_loot,
                &generator,
            )
            .unwrap()
            .unwrap();
        let durable =
            Jukebox::decode(&chunk.block_entities[&POS], &original.registry.items).unwrap();
        assert_eq!(durable.elapsed, Some(1439));
        assert_eq!(durable.record.count, 1);
    }
    drop(original);
    let mut restored = state(Some(dir.clone()));
    crate::tick::sync_chunk_block_entities(&mut restored);
    run(&mut restored, &mut Vec::new(), &mut Vec::new());
    assert!(playing(&restored, POS));
    run(&mut restored, &mut Vec::new(), &mut Vec::new());
    assert!(!playing(&restored, POS));
    assert_eq!(comparator(&restored, POS), Some(11));
    assert_eq!(restored.block_entities[&key(POS)].slots[0].count, 1);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn jukebox_rejects_corrupt_timer_with_exact_diagnostic() {
    let state = state(None);
    let jukebox = Jukebox {
        record: item(&state, "minecraft:music_disc_cat", 1),
        elapsed: Some(1),
    };
    let mut bytes = jukebox.encode(POS, &state.registry.items);
    let Tag::Compound(mut fields) = mc_nbt::read_network(&mut bytes.as_slice()).unwrap() else {
        panic!()
    };
    fields
        .iter_mut()
        .find(|(name, _)| name == "ticks_since_song_started")
        .unwrap()
        .1 = Tag::Int(1);
    bytes.clear();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).unwrap();
    assert!(
        Jukebox::decode(&bytes, &state.registry.items)
            .unwrap_err()
            .to_string()
            .contains("Jukebox.ticks_since_song_started: expected nonnegative long")
    );
}

#[test]
fn playing_clock_skips_idle_wal_and_checkpoint_resumes_elapsed_time() {
    let dir = std::env::temp_dir().join(format!("jukebox-idle-clock-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let actor = joined(&mut original);
    place(&mut original, POS, "minecraft:jukebox");
    let disc = item(&original, "minecraft:music_disc_cat", 1);
    original.players.get_mut(&actor).unwrap().inventory.slots[0] = disc.clone();
    use_on(
        &mut original,
        actor,
        Hand::Main,
        POS,
        &mut ChangeSet::default(),
    );
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let _ = crate::core::journal_entries_for_tick(&mut original, &output);
    let started = original.jukeboxes[&key(POS)].elapsed.unwrap();
    for _ in 0..100 {
        let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut original, &output);
        assert!(!entries.iter().any(|entry| matches!(
            entry,
            crate::journal::JournalEntry::OpaqueBlockEntity { pos, .. }
                | crate::journal::JournalEntry::BlockEntity { pos, .. }
                if *pos == [POS.x, POS.y, POS.z]
        )));
    }
    assert_eq!(original.jukeboxes[&key(POS)].elapsed, Some(started + 100));
    checkpoint(&mut original);
    {
        let capture = original.world.capture_dirty(&BTreeMap::new());
        assert!(!capture.write().unwrap().is_empty());
        let reader = capture.store().unwrap().checkpoint_reader().unwrap();
        let generator =
            mc_worldgen::TerrainGenerator::new(42, Arc::clone(&original.registry.blocks));
        let chunk = reader
            .read_chunk(
                0,
                ChunkPos { x: 0, z: 0 },
                &original.registry.blocks,
                &original.registry.items,
                &original.registry.chest_loot,
                &generator,
            )
            .unwrap()
            .unwrap();
        let durable =
            Jukebox::decode(&chunk.block_entities[&POS], &original.registry.items).unwrap();
        assert_eq!(durable.elapsed, Some(started + 100));
        assert_eq!(durable.record, disc);
    }
    drop(original);
    let mut restored = state(Some(dir.clone()));
    crate::tick::sync_chunk_block_entities(&mut restored);
    assert_eq!(restored.jukeboxes[&key(POS)].elapsed, Some(started + 100));
    assert_eq!(restored.block_entities[&key(POS)].slots[0], disc);
    run(&mut restored, &mut Vec::new(), &mut Vec::new());
    assert_eq!(restored.jukeboxes[&key(POS)].elapsed, Some(started + 101));
    assert!(playing(&restored, POS));
    assert!(restored.opaque_changes.is_empty());
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jukebox_insert_reservation_floor_precedes_song_and_record_effects() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:jukebox");
    let disc = item(&state, "minecraft:music_disc_cat", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = disc.clone();
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "music_disc_cat",
        1,
    );
    let mut changes = ChangeSet::default();
    assert!(use_on(&mut state, actor, Hand::Main, POS, &mut changes));
    assert!(!playing(&state, POS));
    assert!(state.block_entities[&key(POS)].slots[0].is_empty());
    assert_eq!(state.players[&actor].inventory.slots[0], disc);
    assert!(changes.entities_spawned.is_empty());
    crate::plugin_inventories::invalidate_player(&mut state, actor);
    state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 2;
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "music_disc_cat",
        1,
    );
    assert!(use_on(&mut state, actor, Hand::Main, POS, &mut changes));
    assert!(playing(&state, POS));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(state.block_entities[&key(POS)].slots[0], disc);
}
