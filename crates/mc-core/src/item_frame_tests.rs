use super::*;
use crate::boundary::{ConnectionId, PlayerInput};
use crate::tick::run_tick;

struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> (TickState, EntityId) {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).unwrap());
    let world = crate::world::ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 65, 8],
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "frame-player".into(),
            permission_level: 0,
        }],
    );
    state.world.ensure_blocking(&state.desired_chunks());
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [6.5, 65.0, 8.5]);
    state.world.set_block(8, 65, 8, state.registry.states.stone);
    state.world.fold_overlay();
    (state, actor)
}
fn item(state: &TickState, kind: &str, count: i32) -> ItemStack {
    ItemStack::new(
        state
            .registry
            .items
            .id_of(&Identifier::parse(kind).unwrap())
            .unwrap(),
        count,
    )
}
fn place(state: &mut TickState, actor: EntityId, kind: &str) -> EntityId {
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = item(state, kind, 2);
    let (output, _, _) = run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: BlockPos { x: 8, y: 65, z: 8 },
            face: 4,
            cursor: [0.0, 0.5, 0.5],
            sequence: 1,
        }],
    );
    assert_eq!(output.changes.entities_spawned.len(), 1);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    output.changes.entities_spawned[0]
}
fn click(state: &mut TickState, target: EntityId) -> crate::boundary::TickOutput {
    run_tick(
        state,
        &[PlayerInput::InteractEntity {
            connection: ConnectionId(1),
            target,
            hand: Hand::Main,
            location: [0.0, 0.0, 0.0],
        }],
    )
    .0
}

#[test]
fn item_frame_native_place_insert_rotate_pop_and_break_preserve_custody() {
    for kind in ["minecraft:item_frame", "minecraft:glow_item_frame"] {
        let (mut state, actor) = fixture();
        let frame = place(&mut state, actor, kind);
        let slot = state.entities.slot_of(frame).unwrap();
        let mut stack = item(&state, "minecraft:diamond_sword", 1);
        stack.custom_name = Some("displayed".into());
        stack.damage = Some(17);
        state.players.get_mut(&actor).unwrap().inventory.slots[0] = stack.clone();
        click(&mut state, frame);
        assert_eq!(displayed(&state, slot), Some(&stack));
        assert!(state.players[&actor].inventory.slots[0].is_empty());
        for expected in 1..=8 {
            click(&mut state, frame);
            assert_eq!(rotation(&state, slot), expected % 8);
            assert_eq!(
                analog_output(&state, cell(&state, slot), 4),
                Some((expected % 8 + 1) as u8)
            );
        }
        let attack = PlayerInput::Attack {
            connection: ConnectionId(1),
            target: frame,
        };
        let (output, _, _) = run_tick(&mut state, std::slice::from_ref(&attack));
        assert!(state.entities.slot_of(frame).is_some());
        assert!(displayed(&state, slot).is_none());
        assert_eq!(output.changes.entities_spawned.len(), 1);
        let dropped = state
            .entities
            .row(
                state
                    .entities
                    .slot_of(output.changes.entities_spawned[0])
                    .unwrap(),
            )
            .item
            .as_ref()
            .unwrap()
            .full_stack();
        assert_eq!(dropped, stack);
        run_tick(&mut state, &[attack]);
        assert!(state.entities.slot_of(frame).is_none());
    }
}

#[test]
fn item_frame_generation_support_and_wal_recovery() {
    use crate::journal::{JournalWriter, Replay, read_complete_batches_after, replay_batches};
    let (mut state, _) = fixture();
    let position = BlockPos { x: 7, y: 65, z: 8 };
    let elytra = item(&state, "minecraft:elytra", 1);
    assert!(spawn(&mut state, position, 9, false, elytra.clone()).is_none());
    let frame = spawn(&mut state, position, 4, false, elytra.clone()).unwrap();
    assert!(spawn(&mut state, position, 4, false, elytra.clone()).is_none());
    let (mut output, _, _) = run_tick(&mut state, &[]);
    output.changes = Arc::new(ChangeSet {
        entities_spawned: vec![frame],
        ..Default::default()
    });
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let root = std::env::temp_dir().join(format!("solaris-frame-{}", uuid::Uuid::new_v4()));
    let mut writer = JournalWriter::open(&root, 1).unwrap();
    writer.append(1, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&root.join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    let mut corrupt = replay.entities[&frame.0].clone();
    corrupt.appearance.spawn_data = 6;
    assert!(validate_image(&corrupt).is_err());
    corrupt.appearance.spawn_data = 4;
    corrupt
        .appearance
        .extra
        .push((ROTATION, crate::journal::EntityDataValueImage::VarInt(8)));
    assert!(validate_image(&corrupt).is_err());
    let (mut restarted, _) = fixture();
    crate::core::restore_entity(&mut restarted, frame.0, &replay.entities[&frame.0]).unwrap();
    let slot = restarted.entities.slot_of(frame).unwrap();
    assert_eq!(displayed(&restarted, slot), Some(&elytra));
    assert_eq!(
        restarted.entities.body(slot).movement,
        crate::blueprint::MovementClass::Stationary
    );
    let (_, chunks, _) = crate::core::checkpoint_data_for(&restarted, restarted.tick);
    let native = crate::native_checkpoint::encode_entities(
        crate::dimensions::Dimension::Overworld,
        &chunks,
        &restarted.registry.items,
        restarted.tick,
    )
    .unwrap();
    let config = crate::CoreConfig::new(root.join("checkpoint"), 42);
    crate::native_checkpoint::tests::write_manifest(&config, &Default::default()).unwrap();
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let base = store.read_checkpoint().unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: restarted.tick,
                previous_tick: base.tick,
                chunks: &[],
                entities: &native,
                players: &[],
                globals: &base.globals,
            },
            &restarted.registry.blocks,
            &restarted.registry.items,
        )
        .unwrap();
    drop(store);
    let recovered = crate::native_checkpoint::recover_offline(&config).unwrap();
    let native_replay = &recovered.dimensions[&crate::dimensions::Dimension::Overworld];
    let (mut restarted, _) = fixture();
    crate::core::restore_entity(&mut restarted, frame.0, &native_replay.entities[&frame.0])
        .unwrap();
    let slot = restarted.entities.slot_of(frame).unwrap();
    assert_eq!(displayed(&restarted, slot), Some(&elytra));
    assert_eq!(restarted.entities.row(slot).appearance.spawn_data, 4);
    restarted
        .world
        .set_block(8, 65, 8, restarted.registry.states.air);
    let mut drops = Vec::new();
    blocks_changed(
        &mut restarted,
        &[(BlockPos { x: 8, y: 65, z: 8 }, state.registry.states.air)],
        &mut drops,
    );
    assert!(restarted.entities.slot_of(frame).is_none());
    assert_eq!(drops.len(), 2);
    std::fs::remove_dir_all(root).unwrap();
}
