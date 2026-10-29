use super::*;
use crate::boundary::{ConnectionId, EntityId, PlayerInput};
use crate::registry::CoreRegistry;
use crate::tick::run_tick;
use crate::world::ChunkWorld;
use std::collections::BTreeMap;
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
fn fixture(dir: Option<std::path::PathBuf>) -> TickState {
    static REG: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    let registry = Arc::clone(&REG);
    let generator = Arc::new(Empty(registry.states.air));
    let mut world = match dir {
        Some(directory) => ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&crate::config::CoreConfig::new(
                directory, 42,
            ))
            .unwrap(),
            0,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 11],
            mc_world::OVERWORLD_GEOMETRY,
        ),
        None => ChunkWorld::new(
            42,
            generator,
            &registry,
            mc_world::BlockStateId(registry.states.air),
            [8, 201, 11],
            mc_world::OVERWORLD_GEOMETRY,
        ),
    };
    world.ensure_blocking(
        &(-1..=3)
            .flat_map(|x| (-1..=1).map(move |z| (ChunkPos { x, z }, 0)))
            .collect(),
    );
    TickState::new(registry, world, 0, 512)
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
fn frame(state: &mut TickState, count: usize) {
    let water = id(state, "water");
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                state
                    .world
                    .set_block(POS.x + x, POS.y + y, POS.z + z, water);
            }
        }
    }
    let prismarine = id(state, "prismarine");
    let mut placed = 0;
    for x in -2..=2 {
        for y in -2..=2 {
            for z in -2..=2 {
                if frame_cell(x, y, z) {
                    state.world.set_block(
                        POS.x + x,
                        POS.y + y,
                        POS.z + z,
                        if placed < count {
                            prismarine
                        } else {
                            state.registry.states.air
                        },
                    );
                    placed += 1;
                }
            }
        }
    }
    let conduit = id(state, "conduit");
    state.world.set_block(POS.x, POS.y, POS.z, conduit);
    state.world.fold_overlay();
    after_blocks(state, &[(POS, conduit)]);
}
fn player(state: &mut TickState) -> EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: Uuid::new_v4(),
            name: "diver".into(),
            permission_level: 0,
        }],
    );
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 11.5]);
    actor
}
fn move_wet(state: &mut TickState, actor: EntityId, pos: [f64; 3]) {
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, pos);
    let [x, y, z] = pos.map(|v| v.floor() as i32);
    let water = id(state, "water");
    state.world.set_block(x, y, z, water);
    state.world.set_block(x, y + 1, z, water);
    state.world.fold_overlay();
}
#[test]
fn conduit_frame_thresholds_radius_and_water_volume() {
    let mut state = fixture(None);
    for count in [0, 15, 16, 21, 28, 35, 42] {
        frame(&mut state, count);
        assert_eq!(frame_size(&state, POS), count);
    }
    state
        .world
        .set_block(POS.x + 1, POS.y + 1, POS.z + 1, state.registry.states.air);
    state.world.fold_overlay();
    assert_eq!(frame_size(&state, POS), 0);
}
#[test]
fn conduit_grants_wire_effect_and_break_stops_refresh() {
    let mut state = fixture(None);
    let actor = player(&mut state);
    frame(&mut state, 16);
    move_wet(&mut state, actor, [8.5, 201.0, 11.5]);
    state.tick = 40;
    let mut intents = Vec::new();
    run(&mut state, &mut intents);
    let effect = state.players[&actor]
        .active_effects
        .get(EffectId::ConduitPower)
        .unwrap();
    assert_eq!((effect.duration, effect.ambient), (260, true));
    assert_eq!(state.players[&actor].active_effects.dig_multiplier(), 1.2);
    let events = std::mem::take(&mut state.outbox);
    assert!(events.values().flatten().any(|event|matches!(event,PlayerChange::MobEffect{effect,..} if effect.id==EffectId::ConduitPower)));
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .active_effects
        .entries[0]
        .duration = 100;
    frame(&mut state, 15);
    state.tick = 80;
    run(&mut state, &mut intents);
    assert_eq!(
        state.players[&actor]
            .active_effects
            .get(EffectId::ConduitPower)
            .unwrap()
            .duration,
        100
    );
    let player = state.players.get_mut(&actor).unwrap();
    for tick in 0..100 {
        player.active_effects.tick(&mut player.vitals, tick, 20.0);
    }
    assert!(!player.active_effects.has(EffectId::ConduitPower));
}
#[test]
fn conduit_radius_is_32_to_96_and_requires_wet_player() {
    let mut state = fixture(None);
    let actor = player(&mut state);
    frame(&mut state, 16);
    move_wet(&mut state, actor, [41.5, 201.0, 8.5]);
    state.tick = 40;
    run(&mut state, &mut Vec::new());
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::ConduitPower)
    );
    frame(&mut state, 21);
    run(&mut state, &mut Vec::new());
    assert!(
        state.players[&actor]
            .active_effects
            .has(EffectId::ConduitPower)
    );
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .active_effects
        .clear(None);
    let air = state.registry.states.air;
    state.world.set_block(41, 201, 8, air);
    state.world.fold_overlay();
    run(&mut state, &mut Vec::new());
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::ConduitPower)
    );
}
#[test]
fn conduit_full_frame_attacks_hostile_and_syncs_exact_target() {
    let mut state = fixture(None);
    let _ = player(&mut state);
    frame(&mut state, 42);
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:drowned").unwrap())
        .unwrap();
    let mob = state.spawn_mob(type_id, [11.5, 201.0, 8.5], 20.0);
    move_wet(&mut state, mob, [11.5, 201.0, 8.5]);
    state.tick = 40;
    let mut intents = Vec::new();
    run(&mut state, &mut intents);
    assert_eq!(intents.len(), 1);
    state.merge(&mut intents);
    state.commit_merge_effects();
    let slot = state.entities.slot_of(mob).unwrap();
    assert_eq!(state.entities.row(slot).health, 16.0);
    let target = state.entities.row(slot).uuid;
    assert_eq!(image(&state, POS).unwrap(), Some(target));
    let events = std::mem::take(&mut state.outbox);
    assert!(events.values().flatten().any(|event|matches!(event,PlayerChange::BlockEntityData{block_entity_type:25,nbt,..} if nbt==&Tag::Compound(mc_data::conduit_nbt::fields(Some(target))))));
    state.opaque_changes.clear();
    state.tick = 80;
    run(&mut state, &mut Vec::new());
    assert!(
        state.opaque_changes.is_empty(),
        "unchanged target must not rewrite WAL"
    );
    state
        .world
        .set_block(POS.x, POS.y + 2, POS.z + 2, state.registry.states.air);
    state.world.fold_overlay();
    state.tick = 120;
    let mut intents = Vec::new();
    run(&mut state, &mut intents);
    assert!(intents.is_empty());
    assert_eq!(image(&state, POS).unwrap(), None);
}
#[test]
fn conduit_be_wal_checkpoint_restart_preserves_last_target_once() {
    use crate::journal::{
        JournalEntry, JournalWriter, Replay, read_complete_batches_after, replay_batches,
    };
    let dir = tempfile::tempdir().unwrap();
    let mut original = fixture(Some(dir.path().to_path_buf()));
    frame(&mut original, 42);
    let target = Uuid::from_u128((1 << 96) | (2 << 64) | (3 << 32) | 4);
    store(&mut original, POS, Some(target), false);
    let bytes = original
        .world
        .chunk(ChunkPos { x: 0, z: 0 })
        .unwrap()
        .block_entities[&POS]
        .clone();
    let entry = JournalEntry::OpaqueBlockEntity {
        pos: [POS.x, POS.y, POS.z],
        nbt: Some(bytes.clone()),
    };
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(1, 1, &[entry]).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&dir.path().join("journal"), 0).unwrap();
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(replay.opaque_block_entities.len(), 1);
    assert_eq!(
        replay.opaque_block_entities[&(POS.x, POS.y, POS.z)],
        Some(bytes.clone())
    );
    // Native checkpoint supersedes the same WAL image without changing bytes.
    let capture = original.world.capture_dirty(&BTreeMap::new());
    capture.write().unwrap();
    let config = crate::config::CoreConfig::new(dir.path().to_path_buf(), 42);
    let native = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let reader = native.checkpoint_reader().unwrap();
    let persisted = reader
        .read_chunk(
            0,
            ChunkPos { x: 0, z: 0 },
            &original.registry.blocks,
            &original.registry.items,
            &original.registry.chest_loot,
            &Empty(original.registry.states.air),
        )
        .unwrap()
        .unwrap();
    assert_eq!(persisted.block_entities[&POS], bytes);
    drop(reader);
    drop(native);
    drop(capture);
    drop(original);
    let mut restored = fixture(Some(dir.path().to_path_buf()));
    assert_eq!(image(&restored, POS).unwrap(), Some(target));
    validate_loaded(&restored).unwrap();
    for ((x, y, z), nbt) in &replay.opaque_block_entities {
        restored.world.set_opaque_block_entity(
            BlockPos {
                x: *x,
                y: *y,
                z: *z,
            },
            nbt.clone(),
        );
    }
    assert_eq!(image(&restored, POS).unwrap(), Some(target));
    assert_eq!(
        restored
            .world
            .chunk(ChunkPos { x: 0, z: 0 })
            .unwrap()
            .block_entities[&POS],
        bytes
    );
}
#[test]
fn conduit_corrupt_target_wal_and_chunk_fail_closed() {
    use crate::journal::{JournalEntry, JournalWriter, read_complete_batches_after};
    let mut state = fixture(None);
    frame(&mut state, 16);
    let tag = Tag::Compound(vec![
        ("id".into(), Tag::String("minecraft:conduit".into())),
        ("x".into(), Tag::Int(POS.x)),
        ("y".into(), Tag::Int(POS.y)),
        ("z".into(), Tag::Int(POS.z)),
        ("Target".into(), Tag::IntArray(vec![1, 2, 3])),
    ]);
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &tag).unwrap();
    state
        .world
        .set_opaque_block_entity(POS, Some(bytes.clone()));
    assert_eq!(
        validate_loaded(&state).unwrap_err(),
        "conduit Target: expected TAG_Int_Array[4]"
    );
    let dir = tempfile::tempdir().unwrap();
    let config = crate::config::CoreConfig::new(dir.path().to_path_buf(), 42);
    let native = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let pos = ChunkPos { x: 0, z: 0 };
    native
        .import_chunk(
            0,
            state.world.chunk(pos).unwrap(),
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
    let reader = native.checkpoint_reader().unwrap();
    let saved = reader.read_checkpoint().unwrap();
    let counts = native.record_counts().unwrap();
    let stored_error = reader
        .read_chunk(
            0,
            pos,
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &Empty(state.registry.states.air),
        )
        .unwrap_err();
    assert!(matches!(
        &stored_error,
        mc_world::native::NativeError::Chunk(_)
    ));
    assert!(
        stored_error
            .to_string()
            .contains("conduit Target: expected TAG_Int_Array[4]"),
        "{stored_error}"
    );
    let mut unavailable = ChunkWorld::new_native(
        42,
        Arc::clone(&native),
        0,
        Arc::new(Empty(state.registry.states.air)),
        &state.registry,
        mc_world::BlockStateId(state.registry.states.air),
        [POS.x, POS.y, POS.z],
        mc_world::OVERWORLD_GEOMETRY,
    );
    assert!(
        unavailable
            .ensure_blocking(&[(pos, 0)].into_iter().collect())
            .is_empty()
    );
    assert!(!unavailable.has_chunk(pos.x, pos.z));
    let error = unavailable.load_error(pos).unwrap();
    assert_eq!((error.dimension, error.pos), (0, pos));
    assert!(
        error
            .reason
            .contains("conduit Target: expected TAG_Int_Array[4]")
    );
    unavailable.mark_block_entity_dirty(POS);
    assert!(unavailable.capture_dirty(&BTreeMap::new()).write().is_err());
    assert!(unavailable.take_generated(0).is_none());
    let after = native.read_checkpoint().unwrap();
    assert_eq!(after.tick, saved.tick);
    assert_eq!(after.globals, saved.globals);
    assert_eq!(native.record_counts().unwrap(), counts);
    let durable_error = native
        .checkpoint_reader()
        .unwrap()
        .read_chunk(
            0,
            pos,
            &state.registry.blocks,
            &state.registry.items,
            &state.registry.chest_loot,
            &Empty(state.registry.states.air),
        )
        .unwrap_err();
    assert_eq!(durable_error.to_string(), stored_error.to_string());
    assert!(!dir.path().join("region").exists());
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer
        .append(
            1,
            1,
            &[JournalEntry::OpaqueBlockEntity {
                pos: [8, 201, 8],
                nbt: Some(bytes),
            }],
        )
        .unwrap();
    writer.flush().unwrap();
    drop(writer);
    let error = read_complete_batches_after(&dir.path().join("journal"), 0)
        .unwrap_err()
        .to_string();
    assert!(
        error.contains("record 0 offset 32 kind OpaqueBlockEntity"),
        "{error}"
    );
    assert!(
        error.contains("conduit Target: expected TAG_Int_Array[4]"),
        "{error}"
    );
}
#[test]
fn conduit_exposed_rain_grants_power_but_roof_stops_refresh() {
    let mut state = fixture(None);
    let actor = player(&mut state);
    frame(&mut state, 16);
    let slot = state.entities.slot_of(actor).unwrap();
    state.entities.set_pos(slot, [8.5, 201.0, 11.5]);
    state.weather = (crate::commands::Weather::Rain, 1000);
    state.tick = 40;
    run(&mut state, &mut Vec::new());
    assert!(
        state.players[&actor]
            .active_effects
            .has(EffectId::ConduitPower)
    );
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .active_effects
        .clear(None);
    state
        .world
        .set_block(8, 205, 11, state.registry.states.stone);
    state.world.fold_overlay();
    state.tick = 80;
    run(&mut state, &mut Vec::new());
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::ConduitPower)
    );
}
#[test]
fn conduit_power_prevents_native_drowning_without_second_effect() {
    let mut state = fixture(None);
    let actor = player(&mut state);
    frame(&mut state, 16);
    move_wet(&mut state, actor, [8.5, 201.0, 11.5]);
    state.tick = 40;
    run(&mut state, &mut Vec::new());
    state.players.get_mut(&actor).unwrap().vitals.air = 0;
    run_tick(&mut state, &[]);
    // LivingEntity.increaseAirSupply replenishes four units, not all 300 at once.
    assert_eq!(state.players[&actor].vitals.air, 4);
    assert_eq!(state.players[&actor].vitals.health, 20.0);
    assert!(
        !state.players[&actor]
            .active_effects
            .has(EffectId::WaterBreathing)
    );
}
/// Rain stops at motion-blocking blocks (glass, panes), not at light-opaque ones.
#[test]
fn rain_exposure_is_blocked_by_glass_but_not_by_a_torch() {
    let mut state = fixture(None);
    state.weather = (crate::commands::Weather::Rain, 1000);
    let pos = [8.5, 201.0, 11.5];
    assert!(crate::conduit::rain_exposed(&state, pos));
    for (name, exposed) in [
        ("minecraft:torch", true),
        ("minecraft:glass", false),
        ("minecraft:glass_pane", false),
    ] {
        let block = state
            .registry
            .blocks
            .block(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
            .default
            .0;
        state.world.set_block(8, 205, 11, block);
        state.world.fold_overlay();
        assert_eq!(crate::conduit::rain_exposed(&state, pos), exposed, "{name}");
    }
}
