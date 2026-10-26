use super::*;
use crate::boundary::{ChunkPos, ConnectionId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use mc_data::Identifier;
use std::sync::{Arc, LazyLock};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
fn state(dir: Option<std::path::PathBuf>) -> TickState {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REGISTRY);
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    let mut world = if let Some(dir) = dir {
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::CoreConfig::new(dir, 42))
                .unwrap(),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 10],
            mc_world::OVERWORLD_GEOMETRY,
        )
    };
    world.ensure_blocking(&[(ChunkPos { x: 0, z: 0 }, 0)].into_iter().collect());
    let mut state = TickState::new(registry, world, 0, 512);
    state
        .tickets
        .insert("vault-test".into(), (ChunkPos { x: 0, z: 0 }, 0));
    state.refresh_active_chunks(state.tick);
    state
}
fn join(state: &mut TickState, connection: u64) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(connection),
            uuid: uuid::Uuid::new_v4(),
            name: format!("vault{connection}"),
            permission_level: 0,
        }],
    );
    let id = state
        .connections
        .entity_of(ConnectionId(connection))
        .unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
    id
}
fn place(state: &mut TickState, ominous: bool) {
    let id = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:vault").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(POS.x, POS.y, POS.z, id);
    state.world.fold_overlay();
    after_blocks(state, &[(POS, id)]);
    set_properties(
        state,
        POS,
        &[("ominous", if ominous { "true" } else { "false" })],
        &mut Vec::new(),
    );
    state.world.fold_overlay();
}
fn key_stack(state: &TickState, ominous: bool) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(
                &Identifier::parse(if ominous {
                    "minecraft:ominous_trial_key"
                } else {
                    "minecraft:trial_key"
                })
                .unwrap(),
            )
            .unwrap(),
        2,
    )
}
fn step(state: &mut TickState) -> ChangeSet {
    let mut changes = ChangeSet::default();
    run(state, &mut Vec::new(), &mut changes);
    state.world.fold_overlay();
    state.tick += 1;
    changes
}
#[test]
fn vault_consumes_one_key_rewards_once_and_refuses_repeat_player() {
    let mut state = state(None);
    let player = join(&mut state, 1);
    place(&mut state, false);
    let held = key_stack(&state, false);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
    assert!(use_key(
        &mut state,
        player,
        Hand::Main,
        POS,
        &held,
        &mut ChangeSet::default()
    ));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert_eq!(state.vaults[&key(POS)].rewarded.len(), 1);
    assert_eq!(property(&state, POS, "vault_state"), Some("unlocking"));
    let count = state.vaults[&key(POS)].items.len();
    assert!(count > 0);
    let mut drops = Vec::new();
    for _ in 0..300 {
        drops.extend(step(&mut state).entities_spawned);
    }
    assert_eq!(drops.len(), count);
    assert!(use_key(
        &mut state,
        player,
        Hand::Main,
        POS,
        &held,
        &mut ChangeSet::default()
    ));
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert!(state.vaults[&key(POS)].items.is_empty());
    let second = join(&mut state, 2);
    state.players.get_mut(&second).unwrap().inventory.slots[0] = held.clone();
    use_key(
        &mut state,
        second,
        Hand::Main,
        POS,
        &held,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.vaults[&key(POS)].rewarded.len(), 2);
    assert_eq!(state.players[&second].inventory.slots[0].count, 1);
}
#[test]
fn vault_ominous_requires_ominous_key_and_rolls_ominous_reward() {
    let mut state = state(None);
    let player = join(&mut state, 1);
    place(&mut state, true);
    let normal = key_stack(&state, false);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = normal.clone();
    use_key(
        &mut state,
        player,
        Hand::Main,
        POS,
        &normal,
        &mut ChangeSet::default(),
    );
    assert!(state.vaults[&key(POS)].rewarded.is_empty());
    assert_eq!(state.players[&player].inventory.slots[0].count, 2);
    let held = key_stack(&state, true);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
    let uuid = state
        .entities
        .row(state.entities.slot_of(player).unwrap())
        .uuid;
    let seed = state.tick
        ^ uuid.as_u128() as u64
        ^ (POS.x as u64).rotate_left(19)
        ^ (POS.z as u64).rotate_left(37);
    let expected = state
        .registry
        .trial_loot
        .roll(
            "minecraft:chests/trial_chambers/reward_ominous",
            seed,
            &state.registry.items,
            &state.registry.item_facts,
        )
        .unwrap();
    use_key(
        &mut state,
        player,
        Hand::Main,
        POS,
        &held,
        &mut ChangeSet::default(),
    );
    assert_eq!(state.vaults[&key(POS)].items, expected);
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
}
#[test]
fn vault_mid_ejection_checkpoint_retains_remaining_items_and_rewarded_uuid() {
    let dir = std::env::temp_dir().join(format!("vault-checkpoint-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let player = join(&mut original, 1);
    place(&mut original, false);
    let held = key_stack(&original, false);
    original.players.get_mut(&player).unwrap().inventory.slots[0] = held.clone();
    use_key(
        &mut original,
        player,
        Hand::Main,
        POS,
        &held,
        &mut ChangeSet::default(),
    );
    for _ in 0..45 {
        step(&mut original);
    }
    let before = original.vaults[&key(POS)].clone();
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    drop(original);
    let mut restored = state(Some(dir.clone()));
    crate::tick::sync_chunk_block_entities(&mut restored);
    let after = &restored.vaults[&key(POS)];
    assert_eq!(after.rewarded, before.rewarded);
    assert_eq!(after.items, before.items);
    assert_eq!(after.resumes, before.resumes);
    restored.tick = before.resumes;
    let mut drops = Vec::new();
    for _ in 0..300 {
        drops.extend(step(&mut restored).entities_spawned);
    }
    assert_eq!(drops.len(), before.items.len());
    assert!(restored.vaults[&key(POS)].items.is_empty());
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}
#[test]
fn vault_bad_rewarded_uuid_and_ejection_timer_fail_closed() {
    let items = mc_data::items::solaris_required_items();
    let fields = vec![(
        "server_data".into(),
        Tag::Compound(vec![("state_updating_resumes_at".into(), Tag::Long(-1))]),
    )];
    assert_eq!(
        VaultData::from_fields(fields, &items).unwrap_err(),
        "state_updating_resumes_at: expected nonnegative Long"
    );
}
#[test]
fn vault_real_use_item_on_input_consumes_key_in_shared_tick() {
    let mut state = state(None);
    let player = join(&mut state, 1);
    place(&mut state, false);
    let held = key_stack(&state, false);
    state.players.get_mut(&player).unwrap().inventory.slots[0] = held;
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    assert_eq!(state.players[&player].inventory.slots[0].count, 1);
    assert_eq!(state.vaults[&key(POS)].rewarded.len(), 1);
}

#[test]
fn vault_wal_recovers_rewarded_player_pending_rewards_and_key_debit_together() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let dir = std::env::temp_dir().join(format!("vault-wal-{}", uuid::Uuid::new_v4()));
    let mut original = state(Some(dir.clone()));
    let player = join(&mut original, 1);
    place(&mut original, false);
    let uuid = original
        .entities
        .row(original.entities.slot_of(player).unwrap())
        .uuid;
    let held = key_stack(&original, false);
    original.players.get_mut(&player).unwrap().inventory.slots[0] = held;
    original
        .world
        .capture_dirty(&BTreeMap::new())
        .write()
        .unwrap();
    let checkpoint_tick = original.tick;
    let (output, _, _) = run_tick(
        &mut original,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let entries = crate::core::journal_entries_for_tick(&mut original, &output);
    let before = original.vaults[&key(POS)].clone();
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.gate(checkpoint_tick, output.tick).unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), checkpoint_tick).unwrap();
    let once = replay_batches(Replay::default(), &batches);
    let expected = once.opaque_block_entities.clone();
    let twice = replay_batches(once, &batches);
    assert_eq!(twice.opaque_block_entities, expected);
    assert_eq!(twice.players[&uuid.to_string()].inventory[0].count, 1);
    let mut restored = state(Some(dir.clone()));
    restored.tick = twice.tick;
    for (&(x, y, z), &block) in &twice.blocks {
        restored.world.set_block(x, y, z, block);
    }
    restored.world.fold_overlay();
    assert_eq!(property(&restored, POS, "vault_state"), Some("unlocking"));
    for (cell, bytes) in twice.opaque_block_entities {
        restored
            .world
            .set_opaque_block_entity(position(cell), bytes);
    }
    let chunk = restored.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let decoded = decode_chunk(&restored.vaults, &restored.registry, chunk).unwrap();
    restored.vaults.extend(decoded);
    assert_eq!(restored.vaults[&key(POS)].rewarded, before.rewarded);
    assert_eq!(restored.vaults[&key(POS)].items, before.items);
    let mut drops = Vec::new();
    for _ in 0..300 {
        drops.extend(step(&mut restored).entities_spawned);
    }
    assert_eq!(drops.len(), before.items.len());
    assert!(restored.vaults[&key(POS)].rewarded.contains(&uuid));
    drop(writer);
    drop(original);
    drop(restored);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn vault_truncated_resident_record_returns_error_without_tick_panic() {
    let directory = tempfile::tempdir().unwrap();
    let mut state = state(Some(directory.path().to_owned()));
    place(&mut state, false);
    state.world.capture_dirty(&BTreeMap::new()).write().unwrap();
    let chunk_pos = ChunkPos { x: 0, z: 0 };
    let config = crate::CoreConfig::new(directory.path().to_owned(), 42);
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    let counts = store.record_counts().unwrap();
    let generator = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&state.registry.blocks));
    let original = store
        .read_chunk(
            0,
            chunk_pos,
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &generator,
        )
        .unwrap()
        .unwrap();
    state.vaults.remove(&key(POS));
    state
        .world
        .set_opaque_block_entity(POS, Some(vec![mc_nbt::tag_type::COMPOUND]));
    let chunk = state.world.chunk(chunk_pos).unwrap();
    assert!(decode_chunk(&state.vaults, &state.registry, chunk).is_err());
    run_tick(&mut state, &[]);
    assert!(!state.world.has_chunk(0, 0));
    assert_eq!(state.world.load_error(chunk_pos).unwrap().dimension, 0);
    assert_eq!(state.world.load_error(chunk_pos).unwrap().pos, chunk_pos);
    assert!(
        state
            .world
            .load_error(chunk_pos)
            .unwrap()
            .reason
            .contains("vault at")
    );
    assert!(matches!(
        state.world.capture_dirty(&BTreeMap::new()).write(),
        Err(crate::CoreError::Storage(_))
    ));
    let after = store.read_checkpoint().unwrap();
    assert_eq!(after.tick, checkpoint.tick);
    assert_eq!(after.globals, checkpoint.globals);
    assert_eq!(store.record_counts().unwrap(), counts);
    let saved = store
        .read_chunk(
            0,
            chunk_pos,
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &generator,
        )
        .unwrap()
        .unwrap();
    assert_eq!(saved.block_entities, original.block_entities);
    assert_eq!(
        saved.get_block(8, POS.y, 8),
        original.get_block(8, POS.y, 8)
    );
}

#[test]
fn reserved_vault_key_cannot_reward_or_unlock_but_free_stock_can() {
    let mut state = state(None);
    let actor = join(&mut state, 1);
    place(&mut state, false);
    let held = key_stack(&state, false);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = held.clone();
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "trial_key",
        2,
    );
    let before = state.world.block_at(POS.x, POS.y, POS.z);
    let mut changes = ChangeSet::default();
    assert!(use_key(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    assert_eq!(state.players[&actor].inventory.slots[0], held);
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), before);
    assert!(state.vaults[&key(POS)].rewarded.is_empty());
    assert!(state.vaults[&key(POS)].items.is_empty());
    assert!(changes.blocks.is_empty());
    assert!(changes.entities_spawned.is_empty());
    crate::plugin_inventories::invalidate_player(&mut state, actor);
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "trial_key",
        1,
    );
    assert!(use_key(
        &mut state,
        actor,
        Hand::Main,
        POS,
        &held,
        &mut changes
    ));
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(state.vaults[&key(POS)].rewarded.len(), 1);
    assert_eq!(property(&state, POS, "vault_state"), Some("unlocking"));
}
