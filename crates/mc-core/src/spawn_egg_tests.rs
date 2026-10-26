use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::journal::EntityStateImage;

#[test]
fn spawn_egg_villager_and_single_debit_survive_wal_replay_and_native_restart() {
    let (mut state, actor, centre) = crate::hostile_tests::setup(2.0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:villager_spawn_egg").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(item, 2);
    let actor_uuid = state
        .entities
        .row(state.entities.slot_of(actor).unwrap())
        .uuid;
    let (output, _, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos {
                x: centre[0].floor() as i32,
                y: 200,
                z: centre[2].floor() as i32,
            },
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    );
    assert_eq!(output.changes.entities_spawned.len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let id = output.changes.entities_spawned[0];
    let slot = state.entities.slot_of(id).unwrap();
    let uuid = state.entities.row(slot).uuid;
    assert!(state.entities.row(slot).villager.is_some());
    let root = std::env::temp_dir().join(format!("solaris-spawn-egg-{}", uuid::Uuid::new_v4()));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = crate::journal::JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = crate::journal::read_complete_batches_after(&root.join("journal"), 0).unwrap();
    let replay = crate::journal::replay_batches(crate::journal::Replay::default(), &batches);
    let replay = crate::journal::replay_batches(replay, &batches);
    assert_eq!(
        replay.entities.len(),
        1,
        "repeated post-images cannot duplicate the mob"
    );
    assert_eq!(
        replay.players[&actor_uuid.to_string()].inventory[0].count,
        1
    );
    let record = &replay.entities[&id.0];
    assert_eq!(record.uuid, uuid.to_string());
    state.remove_entity(slot);
    crate::core::restore_entity(&mut state, id.0, record).unwrap();
    let slot = state.entities.slot_of(id).unwrap();
    assert!(state.entities.row(slot).villager.is_some());
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let directory = root.join("entities");
    mc_world::anvil::write_entity_regions(&directory, &state.registry.items, &chunks, 0).unwrap();
    let reopened = mc_world::anvil::read_entity_regions(&directory, &state.registry.items).unwrap();
    let natives: Vec<_> = reopened
        .values()
        .flat_map(|chunk| &chunk.entities)
        .filter(|entity| entity.uuid == uuid)
        .collect();
    assert_eq!(natives.len(), 1);
    let native = natives[0];
    assert_eq!(native.wire_id, id.0);
    assert_eq!(native.id, "minecraft:villager");
    let _image = EntityStateImage::from_record(&state.registry, native).unwrap();
    let restored = crate::villager::persistence::villager_from_fields(
        &state.registry.items,
        &native.extra_fields,
        state.tick,
    )
    .unwrap();
    assert_eq!(restored.age, 0);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn spawn_egg_floor_admission_precedes_mob_publication_and_preserves_creative_cost() {
    let (mut state, actor, centre) = crate::hostile_tests::setup(2.0);
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:villager_spawn_egg").unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(item, 1);
    crate::crafter::tests::hold(
        &mut state,
        mc_script::ScriptInventoryEndpoint::PlayerInventory { player_id: 1 },
        "villager_spawn_egg",
        1,
    );
    let clicked = BlockPos {
        x: centre[0].floor() as i32,
        y: 200,
        z: centre[2].floor() as i32,
    };
    let entity_count = state.entities.slots().count();
    let spawned_count = state.hostile.spawned.len();
    assert!(use_on(&mut state, actor, Hand::Main, clicked, 1));
    assert_eq!(state.entities.slots().count(), entity_count);
    assert_eq!(state.hostile.spawned.len(), spawned_count);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        ItemStack::new(item, 1)
    );
    state.players.get_mut(&actor).unwrap().inventory.slots[0].count = 2;
    assert!(use_on(&mut state, actor, Hand::Main, clicked, 1));
    assert_eq!(state.entities.slots().count(), entity_count + 1);
    assert_eq!(state.hostile.spawned.len(), spawned_count + 1);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        ItemStack::new(item, 1)
    );
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(use_on(&mut state, actor, Hand::Main, clicked, 1));
    assert_eq!(state.entities.slots().count(), entity_count + 2);
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        ItemStack::new(item, 1)
    );
}

#[test]
fn fix_mobs_spawn_egg_configures_spawner_without_spawning_adjacent_mob() {
    let (mut state, actor, centre) = crate::hostile_tests::setup(2.0);
    let clicked = BlockPos {
        x: centre[0] as i32,
        y: 200,
        z: centre[2] as i32,
    };
    let spawner = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:spawner").unwrap())
        .unwrap()
        .default
        .0;
    state
        .world
        .set_block(clicked.x, clicked.y, clicked.z, spawner);
    for (name, difficulty) in [
        ("zombie", crate::survival::Difficulty::Normal),
        ("cow", crate::survival::Difficulty::Peaceful),
        ("zombie", crate::survival::Difficulty::Peaceful),
    ] {
        state.difficulty = difficulty;
        let egg = state
            .registry
            .items
            .id_of(&Identifier::parse(format!("minecraft:{name}_spawn_egg")).unwrap())
            .unwrap();
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = ItemStack::new(egg, 2);
        let before = state.entities.len();
        assert!(use_on(&mut state, actor, Hand::Main, clicked, 2));
        assert_eq!(
            state.entities.len(),
            before,
            "spawner click must not spawn an adjacent mob"
        );
        assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
        let (_, Some(bytes)) = state.opaque_changes.last().expect("spawner post-image") else {
            panic!("missing post-image")
        };
        let fields = mc_data::trial_nbt::decode(bytes).unwrap();
        let spawn = mc_data::trial_nbt::compound(&fields, "SpawnData")
            .unwrap()
            .unwrap();
        let entity = mc_data::trial_nbt::compound(spawn, "entity")
            .unwrap()
            .unwrap();
        assert_eq!(
            mc_data::item_nbt::field(entity, "id"),
            Some(&Tag::String(format!("minecraft:{name}")))
        );
    }
}

#[test]
fn fix_mobs_corrupt_spawner_entry_stays_inert_without_tick_panic() {
    let (mut state, _, _) = crate::hostile_tests::setup(2.0);
    let pos = BlockPos { x: 0, y: 200, z: 0 };
    let spawner = state
        .registry
        .blocks
        .block(&Identifier::parse("minecraft:spawner").unwrap())
        .unwrap()
        .default
        .0;
    state.world.set_block(pos.x, pos.y, pos.z, spawner);
    let bytes = mc_data::trial_nbt::encode(
        vec![("SpawnData".into(), Tag::Compound(vec![]))],
        "minecraft:mob_spawner",
        [pos.x, pos.y, pos.z],
    );
    state.world.set_opaque_block_entity(pos, Some(bytes));
    let before = state.opaque_changes.len();
    crate::monster_spawner::after_blocks(&mut state, &[(pos, spawner)]);
    assert_eq!(
        state.opaque_changes.len(),
        before,
        "corrupt spawner must stay inert without executing"
    );
}
