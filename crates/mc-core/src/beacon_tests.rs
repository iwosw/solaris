use crate::boundary::{BlockPos, ChunkPos, ConnectionId, EntityId, PlayerInput};
use crate::effects::EffectId;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;
use mc_nbt::Tag;
use std::sync::{Arc, LazyLock};
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };
struct Empty(u32);
impl mc_world::ChunkGenerator for Empty {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            mc_world::BlockStateId(self.0),
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
        )
    }
}
fn fixture() -> (TickState, EntityId) {
    static REG: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REG);
    let mut world = ChunkWorld::new(
        42,
        Arc::new(Empty(registry.states.air)),
        &registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 201, 11],
        mc_world::OVERWORLD_GEOMETRY,
    );
    world.ensure_blocking(
        &(-1..=3)
            .flat_map(|x| (-1..=1).map(move |z| (ChunkPos { x, z }, 0)))
            .collect(),
    );
    let mut state = TickState::new(registry, world, 0, 512);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "beacon_user".into(),
            permission_level: 0,
        }],
    );
    let entity = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(entity).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 11.5]);
    (state, entity)
}
fn id(state: &TickState, name: &str) -> u32 {
    state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap()
        .default
        .0
}
fn pyramid(state: &mut TickState, levels: i32) {
    let blocks = [
        "iron_block",
        "gold_block",
        "emerald_block",
        "diamond_block",
        "netherite_block",
    ]
    .map(|name| id(state, name));
    for layer in 1..=levels {
        for x in -layer..=layer {
            for z in -layer..=layer {
                state.world.set_block(
                    POS.x + x,
                    POS.y - layer,
                    POS.z + z,
                    blocks[(x + z).unsigned_abs() as usize % blocks.len()],
                );
            }
        }
    }
    let beacon = id(state, "beacon");
    state.world.set_block(POS.x, POS.y, POS.z, beacon);
    let tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:beacon".into())),
        ("x".into(), Tag::Int(POS.x)),
        ("y".into(), Tag::Int(POS.y)),
        ("z".into(), Tag::Int(POS.z)),
        (
            "primary_effect".into(),
            Tag::String("minecraft:speed".into()),
        ),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).unwrap();
    state.world.set_opaque_block_entity(POS, Some(bytes));
    state.world.fold_overlay();
}
#[test]
fn beacon_native_tick_refreshes_selected_effect() {
    let (mut state, entity) = fixture();
    pyramid(&mut state, 1);
    state.tick = 80;
    run_tick(&mut state, &[]);
    assert!(state.players[&entity].active_effects.has(EffectId::Speed));
}

#[test]
fn beacon_mixed_pyramids_and_sky_obstruction() {
    let (mut state, _) = fixture();
    for level in 1..=4 {
        pyramid(&mut state, level);
        assert_eq!(crate::beacon::levels(&state, POS), level);
    }
    let glass = id(&state, "blue_stained_glass");
    state.world.set_block(POS.x, POS.y + 1, POS.z, glass);
    state.world.fold_overlay();
    assert_eq!(crate::beacon::levels(&state, POS), 4);
    let stone = id(&state, "stone");
    state.world.set_block(POS.x, POS.y + 2, POS.z, stone);
    state.world.fold_overlay();
    assert_eq!(crate::beacon::levels(&state, POS), 0);
    state
        .world
        .set_block(POS.x, POS.y + 2, POS.z, state.registry.states.air);
    state.world.set_block(POS.x + 2, POS.y - 2, POS.z, stone);
    state.world.fold_overlay();
    assert_eq!(crate::beacon::levels(&state, POS), 1);
}

#[test]
fn beacon_effect_range_vertical_extension_and_cadence() {
    let (mut state, entity) = fixture();
    pyramid(&mut state, 1);
    let slot = state.entities.slot_of(entity).unwrap();
    for (position, applied) in [
        ([28.5, 300.0, 8.5], true),
        ([29.5, 201.0, 8.5], false),
        ([8.5, 180.0, 8.5], false),
    ] {
        state.entities.set_pos(slot, position);
        state.players.get_mut(&entity).unwrap().active_effects = Default::default();
        state.tick = 80;
        crate::beacon::run(&mut state);
        let effect = state.players[&entity].active_effects.get(EffectId::Speed);
        assert_eq!(effect.is_some(), applied, "{position:?}");
        if let Some(effect) = effect {
            assert_eq!(
                (effect.duration, effect.amplifier, effect.ambient),
                (220, 0, true)
            );
        }
    }
    state.entities.set_pos(slot, [8.5, 201.0, 11.5]);
    state.tick = 81;
    crate::beacon::run(&mut state);
    assert!(!state.players[&entity].active_effects.has(EffectId::Speed));
    let stone = id(&state, "stone");
    state.world.set_block(POS.x, POS.y + 1, POS.z, stone);
    state.world.fold_overlay();
    state.tick = 160;
    crate::beacon::run(&mut state);
    assert!(!state.players[&entity].active_effects.has(EffectId::Speed));
}

fn open(state: &mut TickState) -> Vec<(ConnectionId, Vec<crate::boundary::PlayerChange>)> {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: crate::boundary::Hand::Main,
            position: POS,
            face: 1,
            cursor: [0.5, 1.0, 0.5],
            sequence: 1,
        }],
    )
    .1
}
fn pay(state: &mut TickState, entity: EntityId) {
    let item = state
        .registry
        .items
        .id_of(&mc_data::Identifier::parse("minecraft:iron_ingot").unwrap())
        .unwrap();
    state.players.get_mut(&entity).unwrap().inventory.slots[9] = mc_data::ItemStack::new(item, 1);
    let menu = state.players[&entity].open.as_ref().unwrap();
    let (window, state_id) = (menu.window, menu.state_id);
    run_tick(
        state,
        &[PlayerInput::ContainerClick {
            connection: ConnectionId(1),
            window,
            state_id,
            slot: 1,
            button: 0,
            mode: crate::boundary::ClickMode::QuickMove,
        }],
    );
}

#[test]
fn beacon_menu_payment_tiers_wire_data_and_persistence() {
    use crate::boundary::PlayerChange;
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let (mut state, entity) = fixture();
    pyramid(&mut state, 1);
    let opened = open(&mut state);
    assert!(
        opened
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(change, PlayerChange::MenuOpened { menu_type: 9, .. }))
    );
    assert!(
        opened
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                PlayerChange::MenuProperty {
                    property: 0,
                    value: 1,
                    ..
                }
            ))
    );
    assert!(
        opened
            .iter()
            .flat_map(|(_, changes)| changes)
            .any(|change| matches!(
                change,
                PlayerChange::MenuProperty {
                    property: 1,
                    value: 1,
                    ..
                }
            ))
    );
    pay(&mut state, entity);
    assert_eq!(
        state.players[&entity].open.as_ref().unwrap().grid[0].count,
        1
    );
    assert!(state.players[&entity].inventory.slots[9].is_empty());
    let selection = |primary, secondary| PlayerInput::SetBeacon {
        connection: ConnectionId(1),
        primary: Some(primary as u32),
        secondary,
    };
    state.opaque_changes.clear();
    run_tick(&mut state, &[selection(EffectId::Strength, None)]);
    assert_eq!(
        state.players[&entity].open.as_ref().unwrap().grid[0].count,
        1
    );
    run_tick(&mut state, &[selection(EffectId::Haste, None)]);
    assert!(state.players[&entity].open.as_ref().unwrap().grid[0].is_empty());
    let bytes = state
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&POS]
        .clone();
    assert!(
        state
            .opaque_changes
            .iter()
            .any(|(pos, nbt)| *pos == POS && nbt.as_ref() == Some(&bytes))
    );
    let fields = mc_data::trial_nbt::decode(&bytes).unwrap();
    assert!(fields.contains(&(
        "primary_effect".into(),
        Tag::String("minecraft:haste".into())
    )));
    let dir = tempfile::tempdir().unwrap();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer
        .append(
            1,
            1,
            &[JournalEntry::OpaqueBlockEntity {
                pos: [POS.x, POS.y, POS.z],
                nbt: Some(bytes.clone()),
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let replay = replay_batches(
        Replay::default(),
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    assert_eq!(
        replay.opaque_block_entities[&(POS.x, POS.y, POS.z)],
        Some(bytes.clone())
    );
    let chunk = state.world.chunk(ChunkPos { x: 0, z: 0 }).unwrap();
    let tag = mc_world::anvil::chunk_nbt::chunk_to_nbt(chunk, &state.registry.blocks).unwrap();
    let restored = mc_world::anvil::chunk_nbt::chunk_from_nbt(
        &tag,
        &state.registry.blocks,
        mc_world::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    assert_eq!(restored.block_entities[&POS], bytes);
    let (mut recovered, recovered_player) = fixture();
    pyramid(&mut recovered, 1);
    recovered.world.set_opaque_block_entity(
        POS,
        replay.opaque_block_entities[&(POS.x, POS.y, POS.z)].clone(),
    );
    recovered.tick = 80;
    crate::beacon::run(&mut recovered);
    assert!(
        recovered.players[&recovered_player]
            .active_effects
            .has(EffectId::Haste)
    );
}

#[test]
fn beacon_level_four_secondary_regeneration_and_primary_two() {
    let (mut state, entity) = fixture();
    pyramid(&mut state, 4);
    for secondary in [
        Some(EffectId::Speed as u32),
        Some(EffectId::Regeneration as u32),
    ] {
        open(&mut state);
        pay(&mut state, entity);
        run_tick(
            &mut state,
            &[PlayerInput::SetBeacon {
                connection: ConnectionId(1),
                primary: Some(EffectId::Speed as u32),
                secondary,
            }],
        );
        state.players.get_mut(&entity).unwrap().active_effects = Default::default();
        state.tick = 80;
        crate::beacon::run(&mut state);
        let effects = &state.players[&entity].active_effects;
        let speed = effects.get(EffectId::Speed).unwrap();
        assert_eq!(speed.duration, 340);
        assert_eq!(
            speed.amplifier,
            u8::from(secondary == Some(EffectId::Speed as u32))
        );
        assert_eq!(
            effects.has(EffectId::Regeneration),
            secondary == Some(EffectId::Regeneration as u32)
        );
    }
}
