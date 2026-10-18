use crate::boundary::EntityId;
use crate::dimensions::Dimension;
use crate::effects::{ActiveEffect, EffectId};
use crate::journal::{JournalWriter, read_complete_batches_after, replay_batches};
use crate::tick::{TickState, run_tick};

fn spawn(state: &mut TickState, name: &str, pos: [f64; 3]) -> EntityId {
    let type_id = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    let health = state.registry.blueprint(type_id).unwrap().params.max_health;
    let id = state.spawn_mob(type_id, pos, health);
    state
        .entities
        .set_ground(state.entities.slot_of(id).unwrap(), true);
    id
}

fn checkpoint_restart(state: &TickState, directory: &std::path::Path) -> crate::journal::Replay {
    let (_, chunks, _) = crate::core::checkpoint_data_for(state, state.tick);
    let entities = crate::native_checkpoint::encode_entities(
        Dimension::Overworld,
        &chunks,
        &state.registry.items,
        state.tick,
    )
    .unwrap();
    let config = crate::CoreConfig::new(directory.to_path_buf(), 42);
    crate::native_checkpoint::tests::write_manifest(
        &config,
        &crate::journal::CheckpointManifest {
            tick: state.tick,
            ..Default::default()
        },
    )
    .unwrap();
    let store = crate::native_checkpoint::tests::fixture_store(&config).unwrap();
    let base = store.read_checkpoint().unwrap();
    store
        .commit_checkpoint(
            mc_world::native::NativeCheckpoint {
                tick: state.tick,
                previous_tick: base.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &base.globals,
            },
            &state.registry.blocks,
            &state.registry.items,
        )
        .unwrap();
    drop(store);
    crate::native_checkpoint::recover_offline(&config)
        .unwrap()
        .dimensions
        .remove(&Dimension::Overworld)
        .unwrap()
}

#[test]
fn giant_has_vanilla_attributes_no_ai_and_survives_native_restart_and_anvil_export() {
    let (mut state, player, pos) = crate::hostile_tests::setup(1.5);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    state.players.get_mut(&player).unwrap().vitals.food = 17;
    let id = spawn(&mut state, "giant", pos);
    let slot = state.entities.slot_of(id).unwrap();
    let type_id = match state.entities.body(slot).kind {
        crate::entities::EntityKind::Mob { type_id } => type_id,
        _ => unreachable!(),
    };
    assert!(state.registry.blueprint(type_id).unwrap().goals.is_empty());
    assert_eq!(state.entities.row(slot).health, 100.0);
    assert_eq!(state.entities.row(slot).attack_damage, 50.0);
    assert!((state.entities.body(slot).half_width * 2.0 - 3.6).abs() < 1e-6);
    assert_eq!(state.entities.body(slot).height, 12.0);
    for _ in 0..90 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(
        state.entities.pos(slot),
        pos,
        "giants have neither chase nor wander goals"
    );
    assert_eq!(
        state.players[&player].vitals.health, 20.0,
        "giants have no attack AI"
    );
    state.entities.row_mut(slot).health = 75.0;
    let root = tempfile::tempdir().unwrap();
    let replay = checkpoint_restart(&state, &root.path().join("native"));
    state.remove_entity(slot);
    crate::core::restore_entity(&mut state, id.0, &replay.entities[&id.0]).unwrap();
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.health, 75.0);
    assert_eq!(row.attack_damage, 50.0);
    assert_eq!(
        state
            .entities
            .body(state.entities.slot_of(id).unwrap())
            .height,
        12.0
    );
    assert!(
        (state
            .entities
            .body(state.entities.slot_of(id).unwrap())
            .half_width
            * 2.0
            - 3.6)
            .abs()
            < 1e-6
    );
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let anvil = root.path().join("anvil");
    mc_world::anvil::write_entity_regions(
        &anvil,
        &state.registry.items,
        &chunks,
        state.tick as u32,
    )
    .unwrap();
    let regions = mc_world::anvil::read_entity_regions(&anvil, &state.registry.items).unwrap();
    let entity = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.wire_id == id.0)
        .unwrap();
    assert_eq!(entity.id, "minecraft:giant");
    assert_eq!(entity.health, 75.0);
}

#[test]
fn illusioner_casts_invisibility_and_blindness_and_shoots_durable_bow_arrows() {
    let (mut state, player, pos) = crate::hostile_tests::setup(8.0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let id = spawn(&mut state, "illusioner", pos);
    let slot = state.entities.slot_of(id).unwrap();
    let root = tempfile::tempdir().unwrap();
    // Start with a real native checkpoint, then replay only the casting tail.
    let base = checkpoint_restart(&state, &root.path().join("base"));
    let (output, _, _) = run_tick(&mut state, &[]);
    assert!(
        state
            .entities
            .row(slot)
            .active_effects
            .has(EffectId::Invisibility)
    );
    assert!(
        state.players[&player]
            .active_effects
            .has(EffectId::Blindness)
    );
    assert_ne!(state.entities.row(slot).appearance.shared_flags & 0x20, 0);
    assert!(output.changes.entities_appearance_changed.contains(&id));
    let bow = state.entities.row(slot).appearance.equipment[0]
        .as_ref()
        .unwrap();
    assert_eq!(
        state.registry.items.name_of(bow.item_id).unwrap().as_str(),
        "minecraft:bow"
    );
    assert!(state.entities.slots().any(|s| {
        state.entities.row(s).projectile.as_ref().is_some_and(|p| {
            p.kind == crate::projectiles::ProjectileKind::Arrow
                && p.owner == Some(state.entities.row(slot).uuid)
        })
    }));
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    let mut writer = JournalWriter::open(&root.path().join("wal"), 1).unwrap();
    writer.append(output.tick, output.tick, &entries).unwrap();
    // A refresh with identical flags/particles must still journal its duration.
    state.appearance_changed.clear();
    crate::special_mobs::effect(
        &mut state,
        id,
        ActiveEffect::new(EffectId::Invisibility, 1500, 0),
    );
    assert!(state.appearance_changed.contains(&id));
    let (output, _, _) = run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    writer.append(output.tick, output.tick, &entries).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let batches = read_complete_batches_after(&root.path().join("wal/journal"), base.tick).unwrap();
    let replay = replay_batches(base, &batches);
    assert!(
        replay.entities[&id.0]
            .active_effects
            .has(EffectId::Invisibility)
    );
    let retained = replay.entities[&id.0].active_effects.clone();
    state.remove_entity(slot);
    crate::core::restore_entity(&mut state, id.0, &replay.entities[&id.0]).unwrap();
    let replay = checkpoint_restart(&state, &root.path().join("after"));
    assert_eq!(replay.entities[&id.0].active_effects, retained);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let anvil = root.path().join("anvil");
    mc_world::anvil::write_entity_regions(
        &anvil,
        &state.registry.items,
        &chunks,
        state.tick as u32,
    )
    .unwrap();
    let regions = mc_world::anvil::read_entity_regions(&anvil, &state.registry.items).unwrap();
    let entity = regions
        .values()
        .flat_map(|chunk| &chunk.entities)
        .find(|entity| entity.wire_id == id.0)
        .unwrap();
    assert_eq!(
        entity.active_effects,
        crate::journal::active_effects_to_nbt(&retained)
    );
    assert_eq!(
        crate::entity_loot::equipment_from_fields(&state.registry.items, &entity.extra_fields)
            .unwrap()[0],
        state
            .entities
            .row(state.entities.slot_of(id).unwrap())
            .appearance
            .equipment[0]
    );
}
