use super::*;
use crate::bees::tests::{POS, item, joined, mob, place, state};
use crate::boundary::{ConnectionId, PlayerInput};

#[test]
fn integrity_partial_allay_pickup_preserves_custody_in_wal_recovery() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let mut state = state(None);
    let (mut output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let allay = mob(&mut state, "minecraft:allay", [4.5, 201.0, 6.5]);
    let held = item(&state, "minecraft:stone", 1);
    let slot = state.entities.slot_of(allay).unwrap();
    state.entities.row_mut(slot).allay.as_mut().unwrap().held = held;
    let stack = item(&state, "minecraft:stone", 99);
    let ground = crate::tick::spawn_item_entity(&mut state, [4.5, 201.0, 6.5], stack);
    let ground_slot = state.entities.slot_of(ground).unwrap();
    state.entities.body_mut(ground_slot).pickup_delay = 0;
    output.changes = Arc::new(ChangeSet {
        entities_spawned: vec![allay, ground],
        ..Default::default()
    });
    let birth = crate::core::journal_entries_for_tick(&mut state, &output);
    state.appearance_changed.clear();
    state.tick = 20;
    let mut changes = ChangeSet::default();
    run(&mut state, &mut changes);
    let row = state.entities.row(ground_slot);
    assert_eq!(row.item.as_ref().unwrap().count, 35);
    assert_eq!(
        state
            .entities
            .row(slot)
            .allay
            .as_ref()
            .unwrap()
            .inventory
            .count,
        64
    );
    changes.entities_appearance_changed = std::mem::take(&mut state.appearance_changed);
    output.tick = state.tick;
    output.changes = Arc::new(changes);
    let tail = crate::core::journal_entries_for_tick(&mut state, &output);
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(1, 1, &birth).unwrap();
    writer.append(2, 20, &tail).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    let remaining = replay.entities[&ground.0].item.as_ref().unwrap();
    let collected = &replay.entities[&allay.0]
        .state
        .allay
        .as_ref()
        .unwrap()
        .inventory;
    assert_eq!(remaining.count + collected.count, 99);
    assert_eq!(remaining.count, 35);
    assert_eq!(
        replay.entities[&ground.0]
            .appearance
            .item
            .as_ref()
            .unwrap()
            .count,
        35
    );
    assert_eq!(
        state
            .entities
            .row(ground_slot)
            .appearance
            .item
            .as_ref()
            .unwrap()
            .count,
        35
    );
}

#[test]
fn allay_collects_matching_items_and_delivers_to_player() {
    let mut state = state(None);
    let actor = joined(&mut state);
    let id = mob(&mut state, "minecraft:allay", [4.5, 201.0, 6.5]);
    let diamond = item(&state, "minecraft:diamond", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = diamond.clone();
    let mut changes = ChangeSet::default();
    assert!(interact(&mut state, actor, id, Hand::Main, &mut changes));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let stone_stack = item(&state, "minecraft:stone", 3);
    let stone = crate::tick::spawn_item_entity(&mut state, [4.5, 201.0, 6.5], stone_stack);
    let matching = crate::tick::spawn_item_entity(&mut state, [4.5, 201.0, 6.5], diamond.clone());
    state
        .entities
        .body_mut(state.entities.slot_of(matching).unwrap())
        .pickup_delay = 0;
    state
        .entities
        .body_mut(state.entities.slot_of(stone).unwrap())
        .pickup_delay = 0;
    state.tick = 20;
    run(&mut state, &mut changes);
    assert!(state.entities.slot_of(matching).is_none());
    assert!(state.entities.slot_of(stone).is_some());
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .allay
            .as_ref()
            .unwrap()
            .inventory,
        diamond
    );
    run(&mut state, &mut changes);
    assert!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .allay
            .as_ref()
            .unwrap()
            .inventory
            .is_empty()
    );
    assert_eq!(changes.entities_spawned.len(), 1);
    assert_eq!(changes.entities_removed, vec![matching]);
}
#[test]
fn dancing_duplication_consumes_once_and_cooldown_survives_journal_image() {
    let mut state = state(None);
    let actor = joined(&mut state);
    let id = mob(&mut state, "minecraft:allay", [4.5, 201.0, 6.5]);
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.row_mut(slot).allay.as_mut().unwrap().dancing = true;
    let shard = item(&state, "minecraft:amethyst_shard", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = shard;
    let mut changes = ChangeSet::default();
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    assert_eq!(
        state
            .entities
            .row(slot)
            .allay
            .as_ref()
            .unwrap()
            .duplication_cooldown,
        6000
    );
    let child = state.entities.slot_of(changes.entities_spawned[0]).unwrap();
    assert_eq!(
        state
            .entities
            .row(child)
            .allay
            .as_ref()
            .unwrap()
            .duplication_cooldown,
        6000
    );
    let image = crate::journal::EntityStateImage::from_row(
        state.entities.body(slot),
        state.entities.row(slot),
    );
    let bytes = serde_json::to_vec(&image).unwrap();
    let restored: crate::journal::EntityStateImage = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(restored, image);
    let mut bad = image;
    bad.allay.as_mut().unwrap().duplication_cooldown = 6001;
    assert!(
        serde_json::from_slice::<crate::journal::EntityStateImage>(
            &serde_json::to_vec(&bad).unwrap()
        )
        .unwrap_err()
        .to_string()
        .contains("Allay.DuplicationCooldown: expected 0..=6000")
    );
}
#[test]
fn played_note_block_is_liked_for_thirty_seconds() {
    let mut state = state(None);
    place(&mut state, POS, "minecraft:note_block");
    let id = mob(&mut state, "minecraft:allay", centre(POS));
    note_played(&mut state, POS);
    let slot = state.entities.slot_of(id).unwrap();
    assert_eq!(
        state.entities.row(slot).allay.as_ref().unwrap().note,
        Some(POS)
    );
    for _ in 0..600 {
        state.tick += 1;
        run(&mut state, &mut ChangeSet::default());
    }
    assert!(
        state
            .entities
            .row(slot)
            .allay
            .as_ref()
            .unwrap()
            .note
            .is_none()
    );
}
#[test]
fn player_entity_interaction_dispatch_gives_allay_held_item() {
    let mut state = state(None);
    let actor = joined(&mut state);
    let id = mob(&mut state, "minecraft:allay", [4.5, 201.0, 6.5]);
    let held = item(&state, "minecraft:cobblestone", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held;
    crate::tick::run_tick(
        &mut state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: id,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert_eq!(
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .allay
            .as_ref()
            .unwrap()
            .held
            .count,
        1
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 2);
}

#[test]
fn native_checkpoint_preserves_inventory_equipment_memories_and_cooldown() {
    let mut original = state(None);
    let id = mob(&mut original, "minecraft:allay", centre(POS));
    let slot = original.entities.slot_of(id).unwrap();
    let mut held = item(&original, "minecraft:diamond", 1);
    held.custom_name = Some("keepsake".into());
    let mut carried = held.clone();
    carried.count = 12;
    let value = AllayState {
        held,
        inventory: carried,
        liked_player: Some(uuid::Uuid::new_v4()),
        note: Some(POS),
        note_dimension: "minecraft:overworld".into(),
        note_ticks: 123,
        duplication_cooldown: 4321,
        dancing: true,
        pickup_cooldown: 17,
    };
    original.entities.row_mut(slot).allay = Some(Box::new(value.clone()));
    appearance(&mut original, slot);
    let dir = std::env::temp_dir().join(format!("allay-native-{}", uuid::Uuid::new_v4()));
    let (_, regions, _) = crate::core::checkpoint_data_for(&original, 1);
    mc_world::anvil::entity_regions::write_entity_regions(
        &dir,
        &original.registry.items,
        &regions,
        1,
    )
    .unwrap();
    let native =
        mc_world::anvil::entity_regions::read_entity_regions(&dir, &original.registry.items)
            .unwrap();
    let native = native
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|row| row.id == "minecraft:allay")
        .unwrap();
    assert_eq!(
        from_record(&original.registry.items, native).unwrap(),
        Some(value)
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn jukebox_dance_enables_one_duplicate_and_note_destination_receives_stack() {
    let mut state = state(None);
    let actor = joined(&mut state);
    place(&mut state, POS, "minecraft:jukebox");
    let disc = item(&state, "minecraft:music_disc_cat", 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = disc;
    crate::jukebox::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &mut ChangeSet::default(),
    );
    let id = mob(&mut state, "minecraft:allay", centre(POS));
    state.tick = 20;
    run(&mut state, &mut ChangeSet::default());
    let slot = state.entities.slot_of(id).unwrap();
    assert!(state.entities.row(slot).allay.as_ref().unwrap().dancing);
    let shard = item(&state, "minecraft:amethyst_shard", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = shard;
    let mut changes = ChangeSet::default();
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert_eq!(changes.entities_spawned.len(), 1);
    crate::jukebox::use_on(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &mut ChangeSet::default(),
    );
    let note = BlockPos { x: 6, ..POS };
    place(&mut state, note, "minecraft:note_block");
    note_played(&mut state, note);
    let carried = item(&state, "minecraft:amethyst_shard", 4);
    let value = state.entities.row_mut(slot).allay.as_mut().unwrap();
    value.inventory = carried.clone();
    state.entities.set_pos(slot, centre(note));
    state.tick = 40;
    changes.entities_spawned.clear();
    run(&mut state, &mut changes);
    assert!(!state.entities.row(slot).allay.as_ref().unwrap().dancing);
    assert_eq!(changes.entities_spawned.len(), 1);
    let drop = state
        .entities
        .row(state.entities.slot_of(changes.entities_spawned[0]).unwrap());
    assert_eq!(drop.item.as_ref().unwrap().full_stack(), carried);
}

#[test]
fn duplication_disk_wal_replays_once_and_restart_keeps_cooldown_and_debit() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    use std::sync::Arc;
    let dir = std::env::temp_dir().join(format!("allay-wal-{}", uuid::Uuid::new_v4()));
    let mut original = state(None);
    let actor = joined(&mut original);
    let actor_uuid = original
        .entities
        .row(original.entities.slot_of(actor).unwrap())
        .uuid;
    original.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&original, "minecraft:amethyst_shard", 2);
    let id = mob(&mut original, "minecraft:allay", centre(POS));
    let slot = original.entities.slot_of(id).unwrap();
    let held = item(&original, "minecraft:diamond", 1);
    let row = original.entities.row_mut(slot);
    row.persistent = true;
    row.controls.no_ai = true;
    row.allay.as_mut().unwrap().held = held;
    appearance(&mut original, slot);
    let (mut output, _, _) = crate::tick::run_tick(&mut original, &[]);
    Arc::get_mut(&mut output.changes)
        .expect("tick output uniquely owns its changes")
        .entities_spawned
        .push(id);
    let birth_tick = output.tick;
    let birth = crate::core::journal_entries_for_tick(&mut original, &output);
    original
        .entities
        .row_mut(slot)
        .allay
        .as_mut()
        .unwrap()
        .dancing = true;
    let (output, _, _) = crate::tick::run_tick(
        &mut original,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: id,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert_eq!(output.changes.entities_spawned.len(), 1);
    let child = output.changes.entities_spawned[0];
    let duplication_tick = output.tick;
    let duplication = crate::core::journal_entries_for_tick(&mut original, &output);
    let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
    let cooldown_tick = output.tick;
    let cooldown = crate::core::journal_entries_for_tick(&mut original, &output);
    assert!(cooldown.iter().any(|entry| matches!(
        entry,
        crate::journal::JournalEntry::EntityState { id: saved, .. } if *saved == id.0
    )));
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, birth_tick, &birth).unwrap();
    writer
        .append(duplication_tick, duplication_tick, &duplication)
        .unwrap();
    writer
        .append(cooldown_tick, cooldown_tick, &cooldown)
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    let once = serde_json::to_value(&replay.entities).unwrap();
    let players_once = replay.players.clone();
    let replay = replay_batches(replay, &batches);
    assert_eq!(serde_json::to_value(&replay.entities).unwrap(), once);
    assert_eq!(replay.players, players_once);
    assert_eq!(replay.entities.len(), 2);
    assert_ne!(replay.entities[&id.0].uuid, replay.entities[&child.0].uuid);
    for entity in [id, child] {
        assert_eq!(
            replay.entities[&entity.0]
                .state
                .allay
                .as_ref()
                .unwrap()
                .duplication_cooldown,
            5998
        );
    }
    let player = &replay.players[&actor_uuid.to_string()];
    assert_eq!(player.inventory[0].count, 1);
    let mut invalid = serde_json::to_value(&replay.entities[&id.0]).unwrap();
    invalid["state"]["allay"]["held"]["count"] = serde_json::json!(-1);
    assert!(
        serde_json::from_value::<crate::journal::ReplayEntity>(invalid)
            .unwrap_err()
            .to_string()
            .contains("Allay.Inventory: invalid stack count")
    );
    let mut restored = state(None);
    let actor = joined(&mut restored);
    let slot = restored.entities.slot_of(actor).unwrap();
    restored.entities.row_mut(slot).uuid = actor_uuid;
    restored.players.get_mut(&actor).unwrap().inventory.slots = player.inventory.clone();
    restored.tick = replay.tick;
    for (id, image) in &replay.entities {
        crate::core::restore_entity(&mut restored, *id, image).unwrap();
    }
    let (output, _, _) = crate::tick::run_tick(
        &mut restored,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target: id,
            hand: Hand::Main,
            location: [0.0; 3],
        }],
    );
    assert!(output.changes.entities_spawned.is_empty());
    assert_eq!(restored.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(
        restored
            .entities
            .slots()
            .filter(|slot| restored.entities.row(*slot).allay.is_some())
            .count(),
        2
    );
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn lethal_attack_drops_allay_custody_once_and_disk_wal_replays_once() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    use std::sync::Arc;
    for (mob_drops, held_vanishes) in [(true, false), (false, false), (true, true)] {
        let dir = std::env::temp_dir().join(format!("allay-death-wal-{}", uuid::Uuid::new_v4()));
        let mut original = state(None);
        let _actor = joined(&mut original);
        original
            .gamerules
            .insert("mob_drops".into(), mob_drops.to_string());
        let id = mob(&mut original, "minecraft:allay", centre(POS));
        let slot = original.entities.slot_of(id).unwrap();
        let mut held = item(&original, "minecraft:diamond", 1);
        held.custom_name = Some("held".into());
        let mut carried = item(&original, "minecraft:diamond", 9);
        carried.custom_name = Some("carried".into());
        let vanishing = mc_data::ItemEnchantment {
            id: Identifier::parse("minecraft:vanishing_curse").unwrap(),
            level: 1,
        };
        carried.enchantments.push(vanishing.clone());
        if held_vanishes {
            held.enchantments.push(vanishing);
        }
        let mut expected = vec![carried.clone()];
        if !held_vanishes {
            expected.push(held.clone());
        }
        let row = original.entities.row_mut(slot);
        row.health = 0.01;
        row.protected_until = 0;
        row.persistent = true;
        row.controls.no_ai = true;
        let value = row.allay.as_mut().unwrap();
        value.held = held;
        value.inventory = carried;
        appearance(&mut original, slot);
        let (mut output, _, _) = crate::tick::run_tick(&mut original, &[]);
        Arc::get_mut(&mut output.changes)
            .expect("tick output uniquely owns its changes")
            .entities_spawned
            .push(id);
        let birth_tick = output.tick;
        let birth = crate::core::journal_entries_for_tick(&mut original, &output);
        let (output, _, _) = crate::tick::run_tick(
            &mut original,
            &[PlayerInput::Attack {
                connection: ConnectionId(1),
                target: id,
            }],
        );
        assert!(original.entities.slot_of_any(id).is_none());
        assert_eq!(output.changes.entities_removed, vec![id]);
        assert_eq!(output.changes.entities_spawned.len(), expected.len());
        for stack in &expected {
            assert_eq!(
                output
                    .changes
                    .entities_spawned
                    .iter()
                    .filter(|id| {
                        original
                            .entities
                            .row(original.entities.slot_of(**id).unwrap())
                            .item
                            .as_ref()
                            .unwrap()
                            .full_stack()
                            == *stack
                    })
                    .count(),
                1
            );
        }
        let death_tick = output.tick;
        let death = crate::core::journal_entries_for_tick(&mut original, &output);
        let (output, _, _) = crate::tick::run_tick(&mut original, &[]);
        assert!(output.changes.entities_spawned.is_empty());
        let mut writer = JournalWriter::open(&dir, 1).unwrap();
        writer.append(1, birth_tick, &birth).unwrap();
        writer.append(death_tick, death_tick, &death).unwrap();
        writer.flush().unwrap();
        drop(writer);
        let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
        let replay = replay_batches(Replay::default(), &batches);
        assert!(!replay.entities.contains_key(&id.0));
        assert_eq!(replay.entities.len(), expected.len());
        for stack in &expected {
            assert_eq!(
                replay
                    .entities
                    .values()
                    .filter(|entity| entity.item.as_ref() == Some(stack))
                    .count(),
                1
            );
        }
        let once = serde_json::to_value(&replay.entities).unwrap();
        let replay = replay_batches(replay, &batches);
        assert_eq!(serde_json::to_value(&replay.entities).unwrap(), once);
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[test]
fn collection_sensor_uses_fresh_nearby_chunks_every_twenty_ticks() {
    let mut original = state(None);
    let ids = [
        mob(&mut original, "minecraft:allay", centre(POS)),
        mob(&mut original, "minecraft:allay", centre(POS)),
    ];
    let held = item(&original, "minecraft:diamond", 1);
    for id in ids {
        let slot = original.entities.slot_of(id).unwrap();
        original.entities.row_mut(slot).allay.as_mut().unwrap().held = held.clone();
        appearance(&mut original, slot);
    }
    let matching = item(&original, "minecraft:diamond", 2);
    let near = crate::tick::spawn_item_entity(&mut original, centre(POS), matching);
    let far_stack = item(&original, "minecraft:diamond", 12);
    let far = crate::tick::spawn_item_entity(&mut original, [1000.5, 201.5, 4.5], far_stack);
    for id in [near, far] {
        let slot = original.entities.slot_of(id).unwrap();
        original.entities.body_mut(slot).pickup_delay = 0;
    }
    original.tick = 19;
    run(&mut original, &mut ChangeSet::default());
    assert!(original.entities.slot_of(near).is_some());
    original.tick = 20;
    run(&mut original, &mut ChangeSet::default());
    assert!(original.entities.slot_of_any(near).is_none());
    assert!(original.entities.slot_of(far).is_some());
    assert!(
        !original
            .spawn_census
            .nearby_entities(centre(POS), 32.0)
            .any(|id| id == far)
    );
    // The second allay sees the stale ID remaining in this tick's chunk bucket,
    // revalidates it against the live store, and cannot duplicate its cargo.
    assert_eq!(
        ids.into_iter()
            .map(|id| {
                original
                    .entities
                    .row(original.entities.slot_of(id).unwrap())
                    .allay
                    .as_ref()
                    .unwrap()
                    .inventory
                    .count
            })
            .sum::<i32>(),
        2
    );
    let matching = item(&original, "minecraft:diamond", 3);
    let next = crate::tick::spawn_item_entity(&mut original, centre(POS), matching);
    let slot = original.entities.slot_of(next).unwrap();
    original.entities.body_mut(slot).pickup_delay = 0;
    original.tick = 21;
    run(&mut original, &mut ChangeSet::default());
    assert!(original.entities.slot_of(next).is_some());
    original.tick = 40;
    run(&mut original, &mut ChangeSet::default());
    assert!(original.entities.slot_of_any(next).is_none());
    assert_eq!(
        ids.into_iter()
            .map(|id| {
                original
                    .entities
                    .row(original.entities.slot_of(id).unwrap())
                    .allay
                    .as_ref()
                    .unwrap()
                    .inventory
                    .count
            })
            .sum::<i32>(),
        5
    );
    assert!(original.entities.slot_of(far).is_some());
}

fn seed_advancement_events(state: &mut TickState) {
    use mc_data::advancements::{AdvancementDefinition, AdvancementTree, Criterion};
    let mut tree = AdvancementTree::default();
    tree.definitions.insert(
        "test:allay".into(),
        AdvancementDefinition {
            criteria: [(
                "accepted".into(),
                Criterion {
                    trigger: "minecraft:allay_drop_item_on_block".into(),
                    ..Default::default()
                },
            )]
            .into(),
            ..Default::default()
        },
    );
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(tree);
    state.registry = Arc::new(registry);
    state.advancement_events.clear();
}

#[test]
fn allay_interaction_evidence_requires_an_accepted_transaction() {
    let mut state = state(None);
    let actor = joined(&mut state);
    let id = mob(&mut state, "minecraft:allay", centre(POS));
    seed_advancement_events(&mut state);
    let mut changes = ChangeSet::default();
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert!(state.advancement_events.is_empty());
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        item(&state, "minecraft:diamond", 2);
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert_eq!(state.advancement_events.len(), 1);
    assert_eq!(
        state.advancement_events[0].trigger,
        "minecraft:player_interacted_with_entity"
    );
    assert_eq!(
        state.advancement_events[0].evidence["item"]["id"],
        "minecraft:diamond"
    );
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert_eq!(state.advancement_events.len(), 1);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    interact(&mut state, actor, id, Hand::Main, &mut changes);
    assert_eq!(state.advancement_events.len(), 2);
}

#[test]
fn note_delivery_evidence_requires_actual_delivery_and_near_liked_player() {
    use crate::commands::GameMode;
    for (radius, mode, note, inventory, expected) in [
        (63.9, GameMode::Survival, true, true, true),
        (64.0, GameMode::Survival, true, true, false),
        (65.0, GameMode::Creative, true, true, false),
        (1.0, GameMode::Spectator, true, true, false),
        (1.0, GameMode::Adventure, true, true, false),
        (1.0, GameMode::Survival, false, true, false),
        (1.0, GameMode::Survival, true, false, false),
    ] {
        let mut state = state(None);
        let actor = joined(&mut state);
        seed_advancement_events(&mut state);
        place(&mut state, POS, "minecraft:note_block");
        let mut pos = centre(POS);
        pos[1] += 1.0;
        let id = mob(&mut state, "minecraft:allay", pos);
        let player_slot = state.entities.slot_of(actor).unwrap();
        let uuid = state.entities.row(player_slot).uuid;
        state
            .entities
            .set_pos(player_slot, [pos[0] + radius, pos[1], pos[2]]);
        state.players.get_mut(&actor).unwrap().game_mode = mode;
        let held = item(&state, "minecraft:cake", 1);
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.row_mut(slot).allay = Some(Box::new(AllayState {
            held: held.clone(),
            inventory: if inventory { held } else { ItemStack::EMPTY },
            liked_player: Some(uuid),
            note: note.then_some(POS),
            note_dimension: state.dimension.name().into(),
            note_ticks: 600,
            ..Default::default()
        }));
        let mut changes = ChangeSet::default();
        run(&mut state, &mut changes);
        assert_eq!(
            state.advancement_events.len(),
            usize::from(expected),
            "{radius} {mode:?} {note} {inventory}"
        );
        if expected {
            assert_eq!(changes.entities_spawned.len(), 1);
            let event = &state.advancement_events[0];
            assert_eq!(event.entity, actor);
            assert_eq!(event.trigger, "minecraft:allay_drop_item_on_block");
            assert_eq!(
                event.evidence["location"]["block"]["id"],
                "minecraft:note_block"
            );
            assert_eq!(event.evidence["item"]["id"], "minecraft:cake");
        }
    }
}
