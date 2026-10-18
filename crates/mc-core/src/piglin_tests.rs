use super::*;
use crate::boundary::ChangeSet;
use crate::crafter::tests::{fixture, stack};
use crate::tick::run_tick;

fn spawn(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.spawn_mob(
        type_id,
        pos,
        state.registry.blueprint(type_id).unwrap().params.max_health,
    )
}
fn row(state: &TickState, id: EntityId) -> &ColdRow {
    state.entities.row(state.entities.slot_of(id).unwrap())
}
fn boots(state: &mut TickState, player: EntityId) {
    let gold = stack(state, "golden_boots", 1);
    state.players.get_mut(&player).unwrap().inventory.slots[36] = gold;
}
fn drops(state: &TickState) -> Vec<(EntityId, ItemStack)> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let row = state.entities.row(slot);
            matches!(state.entities.body(slot).kind, EntityKind::Item { .. }).then(|| {
                (
                    state.entities.body(slot).id,
                    row.appearance.item.clone().unwrap(),
                )
            })
        })
        .collect()
}

fn advancement_tree(state: &mut TickState, mut tree: mc_data::advancements::AdvancementTree) {
    tree.rebuild_trigger_index();
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    registry.advancements = Arc::new(tree);
    state.registry = Arc::new(registry);
    state.advancement_events.clear();
}

fn interaction_advancement(state: &mut TickState) {
    let definition = serde_json::from_value(serde_json::json!({
        "criteria": {
            "accepted": {
                "trigger": "minecraft:player_interacted_with_entity",
                "conditions": {
                    "item": {"items": "minecraft:gold_ingot", "count": 2},
                    "entity": {"type": "minecraft:piglin", "flags": {"is_baby": false}}
                }
            }
        },
        "requirements": [["accepted"]]
    }))
    .unwrap();
    advancement_tree(
        state,
        mc_data::advancements::AdvancementTree {
            definitions: [("test:accepted_barter".into(), definition)]
                .into_iter()
                .collect(),
            ..Default::default()
        },
    );
}

#[test]
fn accepted_barter_queues_actual_pre_debit_item_and_piglin_for_either_hand() {
    for (hand, index) in [(Hand::Main, 0), (Hand::Off, 40)] {
        let (mut state, player) = fixture(None);
        interaction_advancement(&mut state);
        let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
        let gold = stack(&state, "gold_ingot", 2);
        state.players.get_mut(&player).unwrap().inventory.slots[index] = gold;
        assert!(interact(&mut state, player, piglin, hand));
        assert_eq!(state.players[&player].inventory.slots[index].count, 1);
        assert!(admiring(row(&state, piglin)));
        assert_eq!(state.advancement_events.len(), 1);
        let event = &state.advancement_events[0];
        assert_eq!(event.entity, player);
        assert_eq!(event.trigger, "minecraft:player_interacted_with_entity");
        assert_eq!(event.evidence["item"]["count"], 2);
        assert_eq!(event.evidence["entity"]["type"], "minecraft:piglin");
        assert_eq!(event.evidence["entity"]["flags"]["is_baby"], false);
        assert_eq!(event.evidence["entity"]["location"]["position"]["z"], 9.5);
        crate::advancements::run(&mut state, &mut ChangeSet::default());
        assert!(
            state.players[&player].advancement_progress["test:accepted_barter"]["criteria"]
                .get("accepted")
                .is_some()
        );
    }
}

#[test]
fn refused_barter_does_not_queue_interaction_or_debit_gold() {
    for refused in ["not_gold", "baby", "brute", "busy", "spectator", "no_ai"] {
        let (mut state, player) = fixture(None);
        interaction_advancement(&mut state);
        let piglin = spawn(
            &mut state,
            if refused == "brute" {
                "piglin_brute"
            } else {
                "piglin"
            },
            [8.5, 201.0, 9.5],
        );
        let held = stack(
            &state,
            if refused == "not_gold" {
                "dirt"
            } else {
                "gold_ingot"
            },
            2,
        );
        state.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
        match refused {
            "baby" => crate::entity_life::make_child(&mut state, piglin),
            "spectator" => {
                state.players.get_mut(&player).unwrap().game_mode =
                    crate::commands::GameMode::Spectator;
            }
            "no_ai" => {
                let slot = state.entities.slot_of(piglin).unwrap();
                state.entities.row_mut(slot).controls.no_ai = true;
            }
            "busy" => {
                assert!(interact(&mut state, player, piglin, Hand::Main));
                state.advancement_events.clear();
                state.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
            }
            _ => {}
        }
        assert!(
            !interact(&mut state, player, piglin, Hand::Main),
            "{refused}"
        );
        assert_eq!(state.players[&player].inventory.slots[0], held, "{refused}");
        assert!(state.advancement_events.is_empty(), "{refused}");
        crate::advancements::run(&mut state, &mut ChangeSet::default());
        assert!(
            state.players[&player]
                .advancement_progress
                .get("test:accepted_barter")
                .is_none(),
            "{refused}"
        );
    }
}

#[test]
#[ignore = "requires local vanilla 26.1.2 advancement sidecars"]
fn vanilla_distract_piglin_grants_on_accepted_gold_from_bare_player() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let mut tree = mc_data::advancements::AdvancementTree::load(&root).unwrap();
    let id = "minecraft:nether/distract_piglin";
    assert!(tree.definitions.contains_key(id));
    tree.definitions.retain(|key, _| key == id);
    for (hand, index) in [(Hand::Main, 0), (Hand::Off, 40)] {
        let (mut state, player) = fixture(None);
        advancement_tree(&mut state, tree.clone());
        assert!(
            state.players[&player].inventory.slots[36..40]
                .iter()
                .all(ItemStack::is_empty)
        );
        let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
        let gold = stack(&state, "gold_ingot", 1);
        state.players.get_mut(&player).unwrap().inventory.slots[index] = gold;
        assert!(interact(&mut state, player, piglin, hand));
        assert!(state.players[&player].inventory.slots[index].is_empty());
        crate::advancements::run(&mut state, &mut ChangeSet::default());
        assert!(
            state.players[&player].advancement_progress[id]["criteria"]
                .get("distract_piglin_directly")
                .is_some()
        );
        assert!(
            state.players[&player].advancement_progress[id]["done"]
                .as_bool()
                .unwrap()
        );
    }
}

#[test]
fn given_gold_consumed_once_admired_for_120_ticks_then_thrown_toward_player() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
    let gold = stack(&state, "gold_ingot", 2);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = gold;
    let start = state.tick;
    assert!(interact(&mut state, player, piglin, Hand::Main));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert!(!interact(&mut state, player, piglin, Hand::Main));
    assert_eq!(
        row(&state, piglin).appearance.equipment[1],
        Some(stack(&state, "gold_ingot", 1))
    );
    for _ in 0..120 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.tick, start + 120);
    assert!(admiring(row(&state, piglin)));
    assert!(drops(&state).is_empty());
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(!admiring(row(&state, piglin)));
    assert!(row(&state, piglin).appearance.equipment[1].is_none());
    let loot = drops(&state);
    assert_eq!(loot.len(), 1);
    assert!(output.changes.entities_spawned.contains(&loot[0].0));
    assert_ne!(loot[0].1.item_id, stack(&state, "gold_ingot", 1).item_id);
    let velocity = state
        .entities
        .vel(state.entities.slot_of(loot[0].0).unwrap());
    assert!(
        velocity[2] > 0.0,
        "loot thrown toward player south of piglin"
    );
}

#[test]
fn dropped_gold_taken_without_dupes_and_non_gold_ignored() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 7.5]);
    let dirt = stack(&state, "dirt", 1);
    let ignored = crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 7.5], dirt);
    let gold = stack(&state, "gold_ingot", 2);
    let payment = crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 7.5], gold);
    for id in [ignored, payment] {
        let slot = state.entities.slot_of(id).unwrap();
        state.entities.body_mut(slot).pickup_delay = 0;
    }
    run_tick(&mut state, &[]);
    assert!(admiring(row(&state, piglin)));
    assert_eq!(row(&state, payment).item.as_ref().unwrap().count, 1);
    assert_eq!(row(&state, ignored).item.as_ref().unwrap().count, 1);
    for _ in 0..119 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(drops(&state).len(), 2);
    run_tick(&mut state, &[]);
    assert_eq!(drops(&state).len(), 3);
    assert_eq!(
        row(&state, payment).appearance.item.as_ref().unwrap().count,
        1
    );
    run_tick(&mut state, &[]);
    assert!(admiring(row(&state, piglin)));
    assert!(state.entities.slot_of(payment).is_none());
}

#[test]
fn baby_and_brute_do_not_barter_and_other_items_do_not_start_admiration() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    let adult = spawn(&mut state, "piglin", [8.5, 201.0, 7.5]);
    let baby = spawn(&mut state, "piglin", [8.5, 201.0, 8.5]);
    crate::entity_life::make_child(&mut state, baby);
    let brute = spawn(&mut state, "piglin_brute", [8.5, 201.0, 9.5]);
    let dirt = stack(&state, "dirt", 3);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = dirt;
    assert!(!interact(&mut state, player, adult, Hand::Main));
    let gold = stack(&state, "gold_ingot", 3);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = gold;
    assert!(!interact(&mut state, player, baby, Hand::Main));
    assert!(!interact(&mut state, player, brute, Hand::Main));
    let payment = stack(&state, "gold_ingot", 1);
    crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 8.5], payment);
    // Do not let the nearby adult take this baby's offered item.
    let slot = state.entities.slot_of(adult).unwrap();
    state.entities.set_pos(slot, [1.5, 201.0, 1.5]);
    for _ in 0..12 {
        run_tick(&mut state, &[]);
    }
    assert!(!admiring(row(&state, baby)));
    assert!(!admiring(row(&state, brute)));
    assert_eq!(state.players[&player].inventory.slots[0].count, 3);
}

#[test]
fn gold_armor_neutrality_and_brute_unconditional_hostility_are_live() {
    for name in ["piglin", "piglin_brute"] {
        let (mut state, player) = fixture(None);
        let player_slot = state.entities.slot_of(player).unwrap();
        state.entities.row_mut(player_slot).protected_until = 0;
        boots(&mut state, player);
        spawn(&mut state, name, [8.5, 201.0, 9.5]);
        let before = state.players[&player].vitals.health;
        run_tick(&mut state, &[]);
        if name == "piglin" {
            assert_eq!(state.players[&player].vitals.health, before);
            state.players.get_mut(&player).unwrap().inventory.slots[36] = ItemStack::EMPTY;
            run_tick(&mut state, &[]);
            assert!(state.players[&player].vitals.health < before);
        } else {
            assert!(state.players[&player].vitals.health < before);
        }
    }
}

#[test]
fn zoglin_attacks_other_mobs_and_hoglin_flees_warped_fungus() {
    let (mut state, player) = fixture(None);
    let player_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(player_slot, [14.5, 201.0, 14.5]);
    let cow = spawn(&mut state, "cow", [8.5, 201.0, 8.5]);
    spawn(&mut state, "zoglin", [8.5, 201.0, 7.5]);
    let health = row(&state, cow).health;
    run_tick(&mut state, &[]);
    assert!(row(&state, cow).health < health);
    let hoglin = spawn(&mut state, "hoglin", [4.5, 201.0, 4.5]);
    let fungus = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:warped_fungus").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(4, 201, 5, fungus);
    run_tick(&mut state, &[]);
    let hoglin_slot = state.entities.slot_of(hoglin).unwrap();
    assert!(
        state.entities.pos(hoglin_slot)[2] < 4.5,
        "hoglin flees north, not toward player south"
    );
}

#[test]
fn zoglin_targets_living_entities_with_separate_ai_owner() {
    let (mut state, player) = fixture(None);
    let player_slot = state.entities.slot_of(player).unwrap();
    state.entities.set_pos(player_slot, [14.5, 201.0, 14.5]);
    let raider = spawn(&mut state, "vindicator", [8.5, 201.0, 8.5]);
    let raider_slot = state.entities.slot_of(raider).unwrap();
    state.entities.row_mut(raider_slot).controls.no_ai = true;
    spawn(&mut state, "zoglin", [8.5, 201.0, 7.5]);
    let health = row(&state, raider).health;
    run_tick(&mut state, &[]);
    assert!(row(&state, raider).health < health);
}

#[test]
fn barter_table_covers_all_19_entries_counts_potions_and_soul_speed() {
    let (state, _) = fixture(None);
    let mut found = std::collections::BTreeSet::new();
    assert_eq!(LOOT.iter().map(|entry| entry.1).sum::<u32>(), 469);
    for seed in 0..8192 {
        let result = reward(&state, seed);
        let name = state.registry.items.name_of(result.item_id).unwrap().path();
        let potion = result
            .extended()
            .potion
            .as_ref()
            .map(|id| id.path().to_owned());
        found.insert((name.to_owned(), potion.clone()));
        let entry = LOOT
            .iter()
            .find(|entry| {
                entry.0 == name
                    && match entry.4 {
                        3 => potion.as_deref() == Some("fire_resistance"),
                        4 => potion.as_deref() == Some("water"),
                        _ => potion.is_none(),
                    }
            })
            .unwrap();
        assert!((entry.2..=entry.3).contains(&result.count));
        if entry.4 == 1 || entry.4 == 2 {
            let enchanted = if entry.4 == 1 {
                &result.extended().stored_enchantments
            } else {
                &result.enchantments
            };
            assert_eq!(enchanted.len(), 1);
            assert_eq!(enchanted[0].id.as_str(), "minecraft:soul_speed");
            assert!((1..=3).contains(&enchanted[0].level));
        }
    }
    assert_eq!(found.len(), 19);
}

#[test]
fn admiring_survives_native_checkpoint_restart_and_anvil_export() {
    use crate::dimensions::Dimension;
    use crate::native_checkpoint::{encode_entities, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;
    let directory = tempfile::tempdir().unwrap();
    let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let (mut state, player) = fixture(None);
    let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
    let gold = stack(&state, "gold_ingot", 1);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = gold;
    assert!(interact(&mut state, player, piglin, Hand::Main));
    state.tick += 37;
    let expected = row(&state, piglin).special_mob.as_deref().cloned();
    let uuid = row(&state, piglin).uuid;
    let tick = state.tick;
    let (_, captured, _) = crate::core::checkpoint_data_for(&state, tick);
    let entities =
        encode_entities(Dimension::Overworld, &captured, &state.registry.items, tick).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: crate::journal::CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = tick;
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &image.globals,
            },
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
    drop(state);
    drop(store);
    let mut restored = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let chunk = crate::boundary::ChunkPos { x: 0, z: 0 };
    restored
        .world
        .ensure_blocking(&[(chunk, 0)].into_iter().collect());
    restored.tickets.insert("piglin-test".into(), (chunk, 0));
    restored.refresh_active_chunks(restored.tick);
    let slot = restored
        .entities
        .slots()
        .find(|slot| restored.entities.row(*slot).uuid == uuid)
        .unwrap();
    let piglin = restored.entities.body(slot).id;
    assert_eq!(
        restored.entities.row(slot).special_mob.as_deref().cloned(),
        expected
    );
    assert_eq!(
        restored.entities.row(slot).appearance.equipment[1],
        Some(stack(&restored, "gold_ingot", 1))
    );
    let (_, exported, _) = crate::core::checkpoint_data_for(&restored, tick);
    let path = directory.path().join("anvil");
    mc_world::anvil::entity_regions::write_entity_regions(
        &path,
        &restored.registry.items,
        &exported,
        tick.try_into().unwrap(),
    )
    .unwrap();
    let reread =
        mc_world::anvil::entity_regions::read_entity_regions(&path, &restored.registry.items)
            .unwrap();
    let native = reread
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.uuid == uuid)
        .unwrap();
    assert_eq!(
        crate::special_mobs::persistence::from_record(native).unwrap(),
        expected
    );
    let Some(Retained::Piglin(barter)) = expected else {
        panic!("barter");
    };
    let mut changes = ChangeSet::default();
    restored.tick = barter.until - 1;
    run(&mut restored, &mut changes);
    assert!(admiring(row(&restored, piglin)));
    assert!(changes.entities_spawned.is_empty());
    restored.tick += 1;
    run(&mut restored, &mut changes);
    assert!(!admiring(row(&restored, piglin)));
    assert_eq!(changes.entities_spawned.len(), 1);
    run(&mut restored, &mut changes);
    assert_eq!(
        changes.entities_spawned.len(),
        1,
        "restart payout is exactly once"
    );
}

#[test]
fn barter_start_remainder_and_payout_survive_checkpoint_plus_disk_wal_restart() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::dimensions::Dimension;
    use crate::journal::{
        CheckpointManifest, JournalWriter, read_complete_batches_after, replay_batches,
    };
    use crate::native_checkpoint::{encode_entities, encode_players, record, refresh_metadata};
    use mc_world::native::NativeCheckpoint;

    for dropped in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let config = crate::CoreConfig::new(directory.path().to_path_buf(), 42);
        let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
        let (mut state, player) = fixture(None);
        boots(&mut state, player);
        let player_uuid = row(&state, player).uuid.to_string();
        let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 7.5]);
        state
            .entities
            .row_mut(state.entities.slot_of(piglin).unwrap())
            .persistent = true;
        let gold = stack(&state, "gold_ingot", 2);
        let payment = if dropped {
            let item = crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 7.5], gold);
            state
                .entities
                .body_mut(state.entities.slot_of(item).unwrap())
                .pickup_delay = 0;
            Some(item)
        } else {
            state.players.get_mut(&player).unwrap().inventory.slots[0] = gold;
            None
        };
        let checkpoint_tick = state.tick;
        let (players, captured, _) = crate::core::checkpoint_data_for(&state, checkpoint_tick);
        let entities = encode_entities(
            Dimension::Overworld,
            &captured,
            &state.registry.items,
            checkpoint_tick,
        )
        .unwrap();
        let players =
            encode_players(players.iter(), &state.registry.items, checkpoint_tick).unwrap();
        let mut image = store.read_checkpoint().unwrap();
        let mut manifest: CheckpointManifest =
            serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
        manifest.tick = checkpoint_tick;
        for child in manifest.dimensions.values_mut() {
            child.tick = checkpoint_tick;
        }
        image.globals.insert(
            "checkpoint".into(),
            record(checkpoint_tick, serde_json::to_vec(&manifest).unwrap()),
        );
        refresh_metadata(&mut image.globals, &manifest).unwrap();
        for global in image.globals.values_mut() {
            global.captured_tick = checkpoint_tick;
        }
        store
            .commit_checkpoint(
                NativeCheckpoint {
                    tick: checkpoint_tick,
                    previous_tick: image.tick,
                    chunks: &[],
                    entities: &entities,
                    players: &players,
                    globals: &image.globals,
                },
                &state.registry.blocks,
                &state.registry.items,
            )
            .unwrap();
        state.appearance_changed.clear();
        let inputs = if dropped {
            Vec::new()
        } else {
            vec![PlayerInput::InteractEntity {
                connection: ConnectionId(1),
                target: piglin,
                hand: Hand::Main,
                location: [0.0; 3],
            }]
        };
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        let (output, _, _) = run_tick(&mut state, &inputs);
        assert!(output.changes.entities_appearance_changed.contains(&piglin));
        if let Some(item) = payment {
            assert!(output.changes.entities_appearance_changed.contains(&item));
            assert_eq!(row(&state, item).item.as_ref().unwrap().count, 1);
        }
        let pending = row(&state, piglin).special_mob.as_deref().cloned();
        assert!(admiring(row(&state, piglin)));
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        writer.append(output.tick, output.tick, &entries).unwrap();
        writer.flush().unwrap();
        let mid = crate::native_checkpoint::recover_with_reader(
            &config,
            Arc::clone(&state.registry),
            store.checkpoint_reader().unwrap(),
        )
        .unwrap();
        let replay = &mid.dimensions[&Dimension::Overworld];
        assert_eq!(replay.entities[&piglin.0].state.special_mob, pending);
        if let Some(item) = payment {
            assert_eq!(replay.entities[&item.0].item.as_ref().unwrap().count, 1);
        } else {
            assert_eq!(replay.players[&player_uuid].inventory[0].count, 1);
        }
        drop(mid);
        let mut reward_id = None;
        for _ in 0..120 {
            let (output, _, _) = run_tick(&mut state, &[]);
            if !output.changes.entities_spawned.is_empty() {
                assert!(output.changes.entities_appearance_changed.contains(&piglin));
                assert!(reward_id.is_none(), "one payout before the second payment");
                assert_eq!(output.changes.entities_spawned.len(), 1);
                reward_id = Some(output.changes.entities_spawned[0]);
            }
            let entries = crate::core::journal_entries_for_tick(&mut state, &output);
            writer.append(output.tick, output.tick, &entries).unwrap();
        }
        let reward_id = reward_id.expect("barter payout");
        assert!(!admiring(row(&state, piglin)));
        let expected_reward = row(&state, reward_id).appearance.item.clone().unwrap();
        writer.flush().unwrap();
        drop(writer);
        drop(state);
        drop(store);

        let mut recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
        let replay = recovered
            .dimensions
            .remove(&Dimension::Overworld)
            .expect("overworld replay");
        assert!(replay.entities[&piglin.0].state.special_mob.is_none());
        assert!(replay.entities[&piglin.0].appearance.equipment[1].is_none());
        assert_eq!(
            replay.entities[&reward_id.0].item.as_ref(),
            Some(&expected_reward)
        );
        if let Some(item) = payment {
            assert_eq!(replay.entities[&item.0].item.as_ref().unwrap().count, 1);
        } else {
            assert_eq!(replay.players[&player_uuid].inventory[0].count, 1);
        }
        let batches =
            read_complete_batches_after(&directory.path().join("journal"), checkpoint_tick)
                .unwrap();
        let once = serde_json::to_value(&replay.entities).unwrap();
        let twice = replay_batches(replay, &batches);
        assert_eq!(serde_json::to_value(&twice.entities).unwrap(), once);
        drop(recovered);

        let mut restarted =
            crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
        if let Some(item) = payment {
            let slot = restarted.entities.slot_of(item).unwrap();
            restarted.entities.set_pos(slot, [1000.5, 201.0, 1000.5]);
        }
        let mut changes = ChangeSet::default();
        run(&mut restarted, &mut changes);
        assert!(
            changes.entities_spawned.is_empty(),
            "completed barter must not pay twice after restart"
        );
        assert!(row(&restarted, piglin).special_mob.is_none());
        assert_eq!(drops(&restarted).len(), if dropped { 2 } else { 1 });
    }
}

#[test]
fn gold_target_rejects_non_piglin_before_gamerule_lookup() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    let cow = spawn(&mut state, "cow", [8.5, 201.0, 7.5]);
    let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 8.5]);
    let gold = stack(&state, "gold_ingot", 1);
    let payment = crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 7.5], gold);
    state
        .entities
        .body_mut(state.entities.slot_of(payment).unwrap())
        .pickup_delay = 0;
    // A disabled pickup gamerule must not matter for a non-piglin: the cheap
    // kind gate rejects the cow before any rule lookup or item scan.
    state
        .gamerules
        .insert("mob_griefing".into(), "false".into());
    let cow_slot = state.entities.slot_of(cow).unwrap();
    assert!(gold_target(&state, cow_slot).is_none());
    // The same disabled rule still blocks a genuinely eligible piglin.
    let piglin_slot = state.entities.slot_of(piglin).unwrap();
    assert!(gold_target(&state, piglin_slot).is_none());
}

#[test]
fn admiring_payout_ignores_non_piglin_kind_gate() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    // Crowd the chunk with non-piglins; only the admiring piglin may pay out.
    for index in 0..8 {
        spawn(&mut state, "cow", [8.5 + f64::from(index), 201.0, 7.5]);
    }
    let piglin = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
    let gold = stack(&state, "gold_ingot", 2);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = gold;
    assert!(interact(&mut state, player, piglin, Hand::Main));
    for _ in 0..=ADMIRE_TICKS {
        run_tick(&mut state, &[]);
    }
    assert!(!admiring(row(&state, piglin)));
    assert_eq!(drops(&state).len(), 1);
}

#[test]
fn no_ai_and_inactive_piglins_do_not_take_dropped_gold() {
    let (mut state, player) = fixture(None);
    boots(&mut state, player);
    let frozen = spawn(&mut state, "piglin", [8.5, 201.0, 7.5]);
    state
        .entities
        .row_mut(state.entities.slot_of(frozen).unwrap())
        .controls
        .no_ai = true;
    let idle = spawn(&mut state, "piglin", [8.5, 201.0, 9.5]);
    let idle_slot = state.entities.slot_of(idle).unwrap();
    state.entities.set_pos(idle_slot, [1000.5, 201.0, 1000.5]);
    assert!(
        !state.world.is_active(
            (1000.5_f64.floor() as i32).div_euclid(16),
            (1000.5_f64.floor() as i32).div_euclid(16),
        ),
        "far piglin must leave the active chunk"
    );
    let gold = stack(&state, "gold_ingot", 2);
    let payment = crate::tick::spawn_item_entity(&mut state, [8.5, 201.0, 7.5], gold);
    state
        .entities
        .body_mut(state.entities.slot_of(payment).unwrap())
        .pickup_delay = 0;
    run_tick(&mut state, &[]);
    assert!(!admiring(row(&state, frozen)));
    assert!(!admiring(row(&state, idle)));
    assert_eq!(row(&state, payment).item.as_ref().unwrap().count, 2);
}
