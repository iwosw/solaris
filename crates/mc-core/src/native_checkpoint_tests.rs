use super::*;
use mc_world::native::{NativeCheckpoint, NativeStore};

static STORES: std::sync::LazyLock<
    std::sync::Mutex<BTreeMap<std::path::PathBuf, std::sync::Weak<NativeStore>>>,
> = std::sync::LazyLock::new(|| std::sync::Mutex::new(BTreeMap::new()));

pub(crate) fn register_store(
    config: &CoreConfig,
    store: &Arc<NativeStore>,
) -> Result<(), CoreError> {
    let mut stores = STORES
        .lock()
        .map_err(|e| CoreError::Storage(format!("native fixture registry poisoned: {e}")))?;
    stores.retain(|_, store| store.strong_count() != 0);
    stores.insert(config.world_dir.clone(), Arc::downgrade(store));
    Ok(())
}

pub(crate) fn fixture_store(config: &CoreConfig) -> Result<Arc<NativeStore>, CoreError> {
    if let Some(store) = STORES
        .lock()
        .map_err(|e| CoreError::Storage(format!("native fixture registry poisoned: {e}")))?
        .get(&config.world_dir)
        .and_then(std::sync::Weak::upgrade)
    {
        return Ok(store);
    }
    let path = config.world_dir.join("native/state.redb");
    let storage = |e: mc_world::native::NativeError| CoreError::Storage(e.to_string());
    if path.exists() {
        let store = Arc::new(NativeStore::open(&path, native_identity(config)).map_err(storage)?);
        register_store(config, &store)?;
        return Ok(store);
    }
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let mut registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    // S1 native settlement startup is world policy: `Core::open` always
    // materializes it before admission, so the fixture checkpoint must persist
    // the same applied identity it will later be reopened against.
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )?;
    let store = Arc::new(NativeStore::create(&path, native_identity(config)).map_err(storage)?);
    register_store(config, &store)?;
    let mut manifest = CheckpointManifest {
        domain_versions: domain_versions(),
        world_spawn: Some([0, 80, 0]),
        ..Default::default()
    };
    for dimension in [Dimension::Nether, Dimension::End] {
        manifest.dimensions.insert(
            dimension,
            Box::new(CheckpointManifest {
                domain_versions: domain_versions(),
                world_spawn: Some([0, 80, 0]),
                ..Default::default()
            }),
        );
    }
    let mut globals = generation_records(config)?;
    globals.insert(
        "checkpoint".into(),
        record(0, serde_json::to_vec(&manifest).unwrap()),
    );
    globals.insert(
        "scoreboard".into(),
        tag_record(0, &crate::scoreboard::Scoreboard::default().to_nbt())?,
    );
    for key in ["level_metadata", "conversion_extras"] {
        globals.insert(key.into(), tag_record(0, &Tag::Compound(Vec::new()))?);
    }
    globals.insert(
        "plugins".into(),
        record(
            0,
            serde_json::to_vec(&crate::plugin::PluginState::default()).unwrap(),
        ),
    );
    globals.insert(
        "settlement_sequence".into(),
        record(0, serde_json::to_vec(&0u64).unwrap()),
    );
    globals.insert(
        "plugin_startup".into(),
        record(0, crate::plugin_startup::world_identity(&registry)?),
    );
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 0,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .map_err(storage)?;
    Ok(store)
}

pub(crate) fn read_manifest(config: &CoreConfig) -> Result<CheckpointManifest, CoreError> {
    let existing = STORES
        .lock()
        .map_err(|e| CoreError::Storage(format!("native fixture registry poisoned: {e}")))?
        .get(&config.world_dir)
        .and_then(std::sync::Weak::upgrade);
    let reader = if let Some(store) = existing {
        store
            .checkpoint_reader()
            .map_err(|e| CoreError::Storage(e.to_string()))?
    } else {
        let store = mc_world::native::NativeReadOnlyStore::open_with_cache(
            &config.world_dir.join("native/state.redb"),
            native_identity(config),
            16 * 1024 * 1024,
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;
        store
            .checkpoint_reader()
            .map_err(|e| CoreError::Storage(e.to_string()))?
    };
    let image = reader
        .read_checkpoint()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    serde_json::from_slice(&image.globals["checkpoint"].payload)
        .map_err(|e| CoreError::Storage(e.to_string()))
}

pub(crate) fn write_manifest(
    config: &CoreConfig,
    manifest: &CheckpointManifest,
) -> Result<(), CoreError> {
    let store = fixture_store(config)?;
    let registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    let mut image = store
        .read_checkpoint()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let mut manifest = manifest.clone();
    manifest.domain_versions = domain_versions();
    manifest.tick = manifest.tick.max(image.tick).max(1);
    let previous: CheckpointManifest = serde_json::from_slice(&image.globals["checkpoint"].payload)
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    for dimension in [Dimension::Nether, Dimension::End] {
        if let std::collections::btree_map::Entry::Vacant(entry) =
            manifest.dimensions.entry(dimension)
        {
            let child = previous.dimensions.get(&dimension).ok_or_else(|| {
                CoreError::Storage(format!("native fixture missing {dimension:?} checkpoint"))
            })?;
            entry.insert(child.clone());
        }
    }
    for child in manifest.dimensions.values_mut() {
        child.tick = manifest.tick;
        child.domain_versions = domain_versions();
    }
    for global in image.globals.values_mut() {
        global.captured_tick = manifest.tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(manifest.tick, serde_json::to_vec(&manifest).unwrap()),
    );
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: manifest.tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &image.entities,
                players: &[],
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .map_err(|e| CoreError::Storage(e.to_string()))
}

#[test]
fn missing_native_database_with_history_points_to_offline_import() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::create_dir(directory.path().join("journal")).unwrap();
    std::fs::write(directory.path().join("journal/batch-1-1.bin"), b"history").unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let error = crate::Core::open(config).unwrap_err().to_string();
    assert!(error.contains("solaris-v2 world import-anvil"), "{error}");
    assert!(!directory.path().join("native/state.redb").exists());
}

#[test]
fn empty_runtime_world_publishes_initial_three_dimension_native_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let core = crate::Core::open(config.clone()).unwrap();
    core.shutdown();
    let recovered = recover_offline(&config).unwrap();
    assert_eq!(recovered.manifest.dimensions.len(), 2);
    assert_eq!(recovered.dimensions.len(), 3);
    assert!(!directory.path().join("CHECKPOINT-MANIFEST.json").exists());
    assert!(!directory.path().join("level.dat").exists());
    assert!(!directory.path().join("dimensions").exists());
    assert!(!directory.path().join("solaris/world.json").exists());
}

#[test]
fn inhabited_time_production_tick_wal_recovers_and_closes_without_players_after_restart() {
    use crate::boundary::{ConnectionId, PlayerInput};
    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_path_buf(), 42);
    config.simulation_distance = 0;
    let _store = fixture_store(&config).unwrap();
    let pos = mc_world::ChunkPos { x: 0, z: 0 };
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    state
        .world
        .ensure_blocking(&[(pos, 0)].into_iter().collect());
    let mut writer = crate::journal::JournalWriter::open(directory.path(), 1).unwrap();
    for tick in 1..=5 {
        let input = if tick == 1 {
            vec![PlayerInput::Join {
                connection: ConnectionId(1),
                uuid: uuid::Uuid::new_v4(),
                name: "inhabitant".into(),
                permission_level: 0,
            }]
        } else {
            Vec::new()
        };
        let (output, _, _) = crate::tick::run_tick(&mut state, &input);
        assert_eq!(output.tick, tick);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        for entry in &entries {
            if let crate::journal::JournalEntry::InhabitedTime { image } = entry {
                assert_eq!(
                    image.tick, output.tick,
                    "boundary uses the committed WAL clock"
                );
            }
        }
        writer.append(output.tick, output.tick, &entries).unwrap();
    }
    writer.flush().unwrap();
    let live = state.world.inhabited_time(pos);
    assert_eq!(live, 5);
    drop(state);

    let mut recovered =
        crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(recovered.tick, 5);
    assert_eq!(recovered.world.inhabited_time(pos), live);
    assert!(
        recovered.players.is_empty(),
        "players are offline after a crash"
    );
    for _ in 0..3 {
        let (output, _, _) = crate::tick::run_tick(&mut recovered, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut recovered, &output);
        writer.append(output.tick, output.tick, &entries).unwrap();
        assert_eq!(
            recovered.world.inhabited_time(pos),
            live,
            "offline intervals close immediately"
        );
        assert!(recovered.world.inhabited_checkpoint(output.tick).is_empty());
    }
    writer.flush().unwrap();
    drop(writer);
    drop(recovered);
    let again = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(again.tick, 8);
    assert_eq!(
        again.world.inhabited_time(pos),
        live,
        "a second crash agrees with live state"
    );
}

#[test]
fn opaque_entity_tags_survive_native_runtime_checkpoint_and_restart() {
    use mc_nbt::Tag;
    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_path_buf(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    config.journal_fsync_every_batches = 1;
    config.view_distance = 1;
    config.simulation_distance = 0;
    let store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let id = state.spawn_mob(state.registry.cow_type_id, [0.5, 80.0, 0.5], 10.0);
    let slot = state.entities.slot_of(id).unwrap();
    let expected = vec![
        ("Silent".into(), Tag::Byte(1)),
        ("NoGravity".into(), Tag::Byte(1)),
        (
            "Tags".into(),
            Tag::List(mc_nbt::ListTag {
                element_type: 8,
                elements: vec![Tag::String("owner-tag".into())],
            }),
        ),
        ("LeftHanded".into(), Tag::Byte(1)),
        ("Glowing".into(), Tag::Byte(1)),
        (
            "plugin:unknown".into(),
            Tag::Compound(vec![("bytes".into(), Tag::ByteArray(vec![-128, 0, 127]))]),
        ),
    ];
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    Arc::make_mut(&mut row.appearance).custom_name_visible = true;
    mc_nbt::write_named(
        &mut row.opaque_entity_fields,
        "",
        &Tag::Compound(expected.clone()),
    )
    .unwrap();
    let uuid = row.uuid;
    let (_, captured, _) = crate::core::checkpoint_data_for(&state, 2);
    let entities =
        encode_entities(Dimension::Overworld, &captured, &state.registry.items, 2).unwrap();
    let mut image = store.read_checkpoint().unwrap();
    let mut manifest: CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = 2;
    for child in manifest.dimensions.values_mut() {
        child.tick = 2;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(2, serde_json::to_vec(&manifest).unwrap()),
    );
    refresh_metadata(&mut image.globals, &manifest).unwrap();
    for global in image.globals.values_mut() {
        global.captured_tick = 2;
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 2,
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
    for _ in 0..2 {
        let core = crate::Core::open(config.clone()).unwrap();
        assert!(core.checkpoint());
        core.shutdown();
        let recovered = recover_offline(&config).unwrap();
        let entity = recovered.dimensions[&Dimension::Overworld]
            .entities
            .values()
            .find(|entity| entity.uuid == uuid.to_string())
            .unwrap();
        assert!(entity.appearance.custom_name_visible);
        let (_, opaque) =
            mc_nbt::read_named(&mut entity.state.opaque_entity_fields.as_slice()).unwrap();
        assert_eq!(opaque, Tag::Compound(expected.clone()));
    }
}

pub(crate) fn write_player(
    config: &CoreConfig,
    uuid: &str,
    player: &crate::journal::ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
) -> Result<(), CoreError> {
    let store = fixture_store(config)?;
    let registry = crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref())?;
    let mut image = store
        .read_checkpoint()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let tick = image
        .tick
        .checked_add(1)
        .ok_or_else(|| CoreError::Storage("fixture tick overflow".into()))?
        .max(player.tick);
    let mut manifest: CheckpointManifest =
        serde_json::from_slice(&image.globals["checkpoint"].payload).unwrap();
    manifest.tick = tick;
    for child in manifest.dimensions.values_mut() {
        child.tick = tick;
    }
    for global in image.globals.values_mut() {
        global.captured_tick = tick;
    }
    image.globals.insert(
        "checkpoint".into(),
        record(tick, serde_json::to_vec(&manifest).unwrap()),
    );
    let mut captured = player.clone();
    captured.tick = tick;
    let uuid = uuid
        .parse::<uuid::Uuid>()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let players = [mc_world::native::NativePlayerSnapshot {
        uuid: *uuid.as_bytes(),
        record: tag_record(tick, &crate::journal::player_to_tag(&captured, items)?)?,
    }];
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &image.entities,
                players: &players,
                globals: &image.globals,
            },
            &registry.blocks,
            items,
        )
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    Ok(())
}

pub(crate) fn read_player(
    config: &CoreConfig,
    uuid: &str,
    items: &mc_data::items::ItemRegistry,
) -> Result<crate::journal::ReplayPlayer, CoreError> {
    let store = fixture_store(config)?;
    let uuid = uuid
        .parse::<uuid::Uuid>()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let reader = store
        .checkpoint_reader()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let image = reader
        .read_checkpoint()
        .map_err(|e| CoreError::Storage(e.to_string()))?;
    let player = image
        .players
        .iter()
        .find(|player| player.uuid == *uuid.as_bytes())
        .ok_or_else(|| CoreError::Storage(format!("native fixture missing player {uuid}")))?;
    crate::journal::player_from_tag(&decode_tag(&player.record, "player")?, items)
}

pub(crate) fn roundtrip_player(
    config: &CoreConfig,
    uuid: &str,
    player: &crate::journal::ReplayPlayer,
    items: &mc_data::items::ItemRegistry,
) -> Result<crate::journal::ReplayPlayer, CoreError> {
    write_player(config, uuid, player, items)?;
    read_player(config, uuid, items)
}

fn roundtrip_native_entity(
    state: &mut crate::tick::TickState,
    id: crate::boundary::EntityId,
) -> mc_data::entity_nbt::VanillaEntity {
    let (_, chunks, _) = crate::core::checkpoint_data_for(state, state.tick);
    let entity = chunks
        .iter()
        .flat_map(|(_, chunk)| &chunk.entities)
        .find(|entity| entity.wire_id == id.0)
        .unwrap()
        .clone();
    let payload = mc_nbt::Tag::Compound(
        mc_data::entity_nbt::entity_to_fields(&state.registry.items, &entity).unwrap(),
    );
    let mut replay = Replay::default();
    crate::dimensions::recovery::hydrate_entity(&state.registry, &mut replay, &payload, state.tick)
        .unwrap();
    state.remove_entity(state.entities.slot_of(id).unwrap());
    crate::core::restore_entity(state, id.0, &replay.entities[&id.0]).unwrap();
    entity
}

#[test]
fn native_projectile_projection_does_not_duplicate_or_resurrect_owned_fields() {
    use mc_nbt::Tag;
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let _store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let id = state.spawn_mob(kind, [0.5, 80.0, 0.5], 10.0);
    let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
    let entity = chunks
        .iter()
        .flat_map(|(_, chunk)| &chunk.entities)
        .find(|entity| entity.wire_id == id.0)
        .unwrap();
    let mut projectile =
        crate::projectiles::persistence::from_entity(entity, &state.registry.items).unwrap();
    projectile.owner = Some(uuid::Uuid::from_u128(91));
    let unknown = ("plugin:arrow".into(), Tag::ByteArray(vec![-128, 0, 127]));
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    row.projectile = Some(Box::new(projectile));
    mc_nbt::write_named(
        &mut row.opaque_entity_fields,
        "",
        &Tag::Compound(vec![unknown.clone()]),
    )
    .unwrap();
    let first = roundtrip_native_entity(&mut state, id);
    let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
    let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice()).unwrap();
    assert_eq!(opaque, Tag::Compound(vec![unknown.clone()]));
    row.projectile.as_mut().unwrap().owner = None;
    let mut stale = vec![unknown.clone()];
    stale.extend(
        first.extra_fields.into_iter().filter(|(key, _)| {
            matches!(key.as_str(), "Owner" | "life" | "solaris:ProjectileRuntime")
        }),
    );
    row.opaque_entity_fields.clear();
    mc_nbt::write_named(&mut row.opaque_entity_fields, "", &Tag::Compound(stale)).unwrap();
    let second = roundtrip_native_entity(&mut state, id);
    assert!(!second.extra_fields.iter().any(|(key, _)| key == "Owner"));
    assert_eq!(
        second
            .extra_fields
            .iter()
            .filter(|(key, _)| key == "life")
            .count(),
        1
    );
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    assert_eq!(row.projectile.as_ref().unwrap().owner, None);
    let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice()).unwrap();
    assert_eq!(opaque, Tag::Compound(vec![unknown]));
}

#[test]
fn native_villager_projection_does_not_resurrect_absent_optional_fields() {
    use mc_nbt::Tag;
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let _store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let kind = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:zombie_villager").unwrap())
        .unwrap();
    let id = state.spawn_mob(kind, [0.5, 80.0, 0.5], 10.0);
    let unknown = ("plugin:villager".into(), Tag::String("retained".into()));
    let slot = state.entities.slot_of(id).unwrap();
    let row = state.entities.row_mut(slot);
    row.villager = Some(Box::new(crate::villager::VillagerState {
        home: Some(mc_world::BlockPos { x: 0, y: 80, z: 0 }),
        sleeping: true,
        conversion_time: Some(40),
        conversion_player: Some(uuid::Uuid::from_u128(92)),
        ..Default::default()
    }));
    mc_nbt::write_named(
        &mut row.opaque_entity_fields,
        "",
        &Tag::Compound(vec![unknown.clone()]),
    )
    .unwrap();
    let first = roundtrip_native_entity(&mut state, id);
    let row = state.entities.row_mut(state.entities.slot_of(id).unwrap());
    let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice()).unwrap();
    assert_eq!(opaque, Tag::Compound(vec![unknown.clone()]));
    let villager = row.villager.as_mut().unwrap();
    villager.sleeping = false;
    villager.conversion_time = Some(-1);
    villager.conversion_player = None;
    let mut stale = vec![unknown.clone()];
    stale.extend(
        first
            .extra_fields
            .into_iter()
            .filter(|(key, _)| matches!(key.as_str(), "sleeping_pos" | "ConversionPlayer")),
    );
    row.opaque_entity_fields.clear();
    mc_nbt::write_named(&mut row.opaque_entity_fields, "", &Tag::Compound(stale)).unwrap();
    let second = roundtrip_native_entity(&mut state, id);
    assert!(
        !second
            .extra_fields
            .iter()
            .any(|(key, _)| { matches!(key.as_str(), "sleeping_pos" | "ConversionPlayer") })
    );
    let row = state.entities.row(state.entities.slot_of(id).unwrap());
    let villager = row.villager.as_ref().unwrap();
    assert!(!villager.sleeping);
    assert_eq!(villager.conversion_player, None);
    let (_, opaque) = mc_nbt::read_named(&mut row.opaque_entity_fields.as_slice()).unwrap();
    assert_eq!(opaque, Tag::Compound(vec![unknown]));
}

#[test]
fn native_replay_rejects_malformed_opaque_entity_fields_before_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let _store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let id = state.spawn_mob(state.registry.cow_type_id, [0.5, 80.0, 0.5], 10.0);
    let entity = roundtrip_native_entity(&mut state, id);
    let payload = mc_nbt::Tag::Compound(
        mc_data::entity_nbt::entity_to_fields(&state.registry.items, &entity).unwrap(),
    );
    let mut replay = Replay::default();
    crate::dimensions::recovery::hydrate_entity(&state.registry, &mut replay, &payload, state.tick)
        .unwrap();
    let record = replay.entities.get_mut(&id.0).unwrap();
    for payload in [vec![8, 0, 0, 0, 0], vec![10, 0, 0, 0, 0]] {
        record.state.opaque_entity_fields = payload;
        let error = crate::dimensions::recovery::validate_entity(&state, id.0, record).unwrap_err();
        assert!(error.to_string().contains("opaque entity fields:"));
    }
}

#[test]
fn native_entity_payload_uuid_must_match_record_key_without_rewriting_custody() {
    use mc_world::native::NativeEntitySnapshot;

    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let store = fixture_store(&config).unwrap();
    write_manifest(&config, &read_manifest(&config).unwrap()).unwrap();
    let image = store.read_checkpoint().unwrap();
    let identity = uuid::Uuid::from_u128(0xc0570d1);
    let payload_uuid = uuid::Uuid::from_u128(0xc0570d2);
    let entity = mc_data::entity_nbt::VanillaEntity {
        id: "minecraft:cow".into(),
        uuid: payload_uuid,
        wire_id: 7,
        pos: [-527.5, 80.0, 1040.5],
        ..Default::default()
    };
    let entities = [NativeEntitySnapshot {
        identity: *identity.as_bytes(),
        dimension: 0,
        position: mc_world::ChunkPos { x: -33, z: 65 },
        generated_slot: None,
        journal_lsn: image.tick,
        payload: Some(Tag::Compound(
            mc_data::entity_nbt::entity_to_fields(&registry.items, &entity).unwrap(),
        )),
    }];
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: image.tick,
                previous_tick: image.tick,
                chunks: &[],
                entities: &entities,
                players: &[],
                globals: &image.globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    let before = store.read_checkpoint().unwrap();
    drop(store);

    let error = crate::Core::open(config.clone()).unwrap_err().to_string();
    for detail in [
        config
            .world_dir
            .join("native/state.redb")
            .display()
            .to_string(),
        "minecraft:overworld".into(),
        "chunk (-33, 65)".into(),
        "entity wire-id 7".into(),
        payload_uuid.to_string(),
        identity.to_string(),
        "minecraft:cow".into(),
        "differs from native entity record key".into(),
    ] {
        assert!(error.contains(&detail), "missing {detail:?} in {error}");
    }
    let after = fixture_store(&config).unwrap().read_checkpoint().unwrap();
    assert_eq!(after.tick, before.tick, "refusal cannot advance checkpoint");
    assert_eq!(after.globals.len(), before.globals.len());
    for (key, expected) in &before.globals {
        let actual = &after.globals[key];
        assert_eq!(actual.version, expected.version, "{key}");
        assert_eq!(actual.captured_tick, expected.captured_tick, "{key}");
        assert_eq!(actual.payload, expected.payload, "{key}");
    }
    assert!(before.players.is_empty() && after.players.is_empty());
    assert_eq!(before.entities.len(), 1);
    assert_eq!(after.entities.len(), 1);
    let expected = &before.entities[0];
    let actual = &after.entities[0];
    assert_eq!(actual.identity, expected.identity);
    assert_eq!(actual.dimension, expected.dimension);
    assert_eq!(actual.position, expected.position);
    assert_eq!(actual.generated_slot, expected.generated_slot);
    assert_eq!(actual.journal_lsn, expected.journal_lsn);
    assert_eq!(
        actual.payload, expected.payload,
        "refusal cannot rewrite custody"
    );
}

#[tokio::test]
async fn native_save_ack_publishes_durable_checkpoint_before_shutdown() {
    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_path_buf(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    let core = crate::Core::open(config.clone()).unwrap();
    let captured_tick =
        tokio::time::timeout(std::time::Duration::from_secs(10), core.subscribe().recv())
            .await
            .unwrap()
            .unwrap()
            .tick;
    assert!(core.checkpoint(), "save acknowledgment must succeed");
    let store = fixture_store(&config).unwrap();
    // Read one committed transaction while the runtime writer remains live.
    // Shutdown cannot supply the checkpoint this acknowledgment promises.
    let reader = store.checkpoint_reader().unwrap();
    let saved = reader.read_checkpoint().unwrap();
    assert!(saved.tick >= captured_tick);
    let manifest: CheckpointManifest =
        serde_json::from_slice(&saved.globals["checkpoint"].payload).unwrap();
    assert_eq!(manifest.tick, saved.tick);
    assert!(
        manifest
            .dimensions
            .values()
            .all(|child| child.tick == saved.tick)
    );
    drop(reader);
    core.shutdown();
}

#[test]
fn custom_name_visible_survives_native_store_restart_without_opaque_shadow() {
    use crate::boundary::{ChangeSet, ConnectionId};
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_owned(), 42);
    let (mut state, actor, _) = crate::hostile_tests::setup(8.0);
    state.gamerules.insert("spawn_mobs".into(), "false".into());
    let command = crate::commands::parse(
        "summon cow ~ ~ ~ {NoAI:1b,CustomName:'Durable Cow',CustomNameVisible:1b}",
        true,
    )
    .unwrap();
    let mut changes = ChangeSet::default();
    crate::command_execution::execute(&mut state, ConnectionId(1), actor, &command, &mut changes);
    let id = changes.entities_spawned[0];
    for visible in [true, false] {
        let slot = state.entities.slot_of(id).unwrap();
        Arc::make_mut(&mut state.entities.row_mut(slot).appearance).custom_name_visible = visible;
        state.appearance_changed.push(id);
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        let entries = crate::core::journal_entries_for_tick(&mut state, &output);
        let replay =
            crate::journal::replay_batches(Replay::default(), &[(1, output.tick, entries)]);
        assert_eq!(
            replay.entities[&id.0].appearance.custom_name_visible,
            visible
        );
        let store = fixture_store(&config).unwrap();
        let mut previous = store.read_checkpoint().unwrap();
        let tick = previous.tick + 1;
        let mut manifest: CheckpointManifest =
            serde_json::from_slice(&previous.globals["checkpoint"].payload).unwrap();
        manifest.tick = tick;
        for child in manifest.dimensions.values_mut() {
            child.tick = tick;
        }
        for global in previous.globals.values_mut() {
            global.captured_tick = tick;
        }
        previous.globals.insert(
            "checkpoint".into(),
            record(tick, serde_json::to_vec(&manifest).unwrap()),
        );
        let (_, chunks, _) = crate::core::checkpoint_data_for(&state, state.tick);
        let entities =
            encode_entities(state.dimension, &chunks, &state.registry.items, tick).unwrap();
        store
            .commit_checkpoint(
                NativeCheckpoint {
                    tick,
                    previous_tick: previous.tick,
                    chunks: &[],
                    entities: &entities,
                    players: &[],
                    globals: &previous.globals,
                },
                &state.registry.blocks,
                &state.registry.items,
            )
            .unwrap();
        drop(store);
        let reopened = NativeStore::open(
            &config.world_dir.join("native/state.redb"),
            native_identity(&config),
        )
        .unwrap();
        let saved = reopened.read_checkpoint().unwrap();
        let uuid = state.entities.row(slot).uuid;
        let payload = saved
            .entities
            .iter()
            .find(|entity| entity.identity == *uuid.as_bytes())
            .unwrap()
            .payload
            .as_ref()
            .unwrap();
        let Tag::Compound(fields) = payload else {
            panic!("native entity compound")
        };
        assert_eq!(
            fields
                .iter()
                .filter(|(key, _)| key == "CustomNameVisible")
                .count(),
            1
        );
        assert!(fields.contains(&("CustomNameVisible".into(), Tag::Byte(i8::from(visible)))));
        let mut recovered = Replay::default();
        crate::dimensions::recovery::hydrate_entity(&state.registry, &mut recovered, payload, tick)
            .unwrap();
        let restored = &recovered.entities[&id.0];
        assert_eq!(restored.appearance.custom_name_visible, visible);
        assert!(restored.state.opaque_entity_fields.is_empty());
        state.remove_entity(slot);
        crate::core::restore_entity(&mut state, id.0, restored).unwrap();
        assert_eq!(
            state
                .entities
                .row(state.entities.slot_of(id).unwrap())
                .appearance
                .custom_name_visible,
            visible
        );
    }
}

#[test]
fn statistics_native_checkpoint_restart_and_wal_postimage_restore() {
    use crate::journal::{JournalEntry, JournalWriter};
    use mc_data::statistics::{CustomStat, StatType};

    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_path_buf(), 42);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let uuid = uuid::Uuid::from_u128(0x57544154);
    let input = serde_json::json!({
        "DataVersion": 4100,
        "retained_large_unicode": "я".repeat(40_000),
        "stats": {
            "minecraft:custom": {"minecraft:jump": 3, "future:counter": 55},
            "minecraft:mined": {"minecraft:stone": 7},
            "future:category": {"future:counter": [1, 2, null]}
        }
    });
    let mut player =
        crate::journal::player_from_tag(&Tag::Compound(Vec::new()), &registry.items).unwrap();
    player.statistics = statistics_from_json(input.clone(), &registry).unwrap();
    let stone = registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .raw_id as u32;
    let saved = roundtrip_player(&config, &uuid.to_string(), &player, &registry.items).unwrap();
    assert_eq!(saved.statistics, player.statistics);
    assert_eq!(
        statistics_to_json(&saved.statistics, &registry).unwrap(),
        input
    );
    let restarted = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(
        restarted.offline_players[&uuid].statistics,
        player.statistics
    );
    drop(restarted);

    let mut statistics = player.statistics.clone();
    statistics.increment_custom(CustomStat::Jump, 2);
    statistics.set_custom(CustomStat::TimeSinceRest, 0);
    statistics.increment(StatType::Mined, stone, 4);
    let entry = JournalEntry::Player {
        uuid: uuid.to_string(),
        opaque_player_fields: player.opaque_player_fields.clone(),
        statistics: Box::new(statistics.clone()),
        advancement_progress: None,
        unlocked_recipes: None,
        dimension: player.dimension,
        pos: player.pos,
        yaw: player.yaw,
        pitch: player.pitch,
        health: player.health,
        food: player.food,
        saturation: player.saturation,
        exhaustion: player.exhaustion,
        air: player.air,
        experience_level: player.experience_level,
        experience_progress: player.experience_progress,
        experience_total: player.experience_total,
        game_mode: player.game_mode,
        selected: player.selected,
        inventory: player.inventory.clone(),
        carried: player.carried.clone(),
        grid: player.grid.clone(),
        respawn: player.respawn,
        respawn_dimension: player.respawn_dimension,
        respawn_forced: player.respawn_forced,
        respawn_yaw: player.respawn_yaw,
        respawn_pitch: player.respawn_pitch,
        active_effects: player.active_effects.clone(),
        enchantment_seed: player.enchantment_seed,
        warden_warning: player.warden_warning,
        current_impulse_impact_pos: player.current_impulse_impact_pos,
        fall_flying: player.fall_flying,
        burn_ticks: player.burn_ticks,
        portal: player.portal,
        tick: 2,
    };
    {
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(2, 2, std::slice::from_ref(&entry)).unwrap();
    }
    let restarted = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restarted.offline_players[&uuid].statistics, statistics);
    assert_eq!(
        restarted.offline_players[&uuid]
            .statistics
            .get_custom(CustomStat::Jump),
        5
    );
    assert_eq!(
        restarted.offline_players[&uuid]
            .statistics
            .get(StatType::Mined, stone),
        11
    );
    assert_eq!(
        read_player(&config, &uuid.to_string(), &registry.items)
            .unwrap()
            .statistics,
        player.statistics
    );
    let mut old = serde_json::to_value(entry).unwrap();
    old["Player"].as_object_mut().unwrap().remove("statistics");
    let old: JournalEntry = serde_json::from_value(old).unwrap();
    let old = crate::journal::replay_batches(Replay::default(), &[(2, 2, vec![old])]);
    assert_eq!(
        old.players[&uuid.to_string()].statistics,
        Default::default()
    );
    let base = restarted.offline_players[&uuid].clone();
    drop(restarted);
    statistics.advance_tick(3);
    statistics.set_active(true, true, false, false);
    statistics.set_custom(CustomStat::Jump, 0);
    statistics.increment(StatType::Used, 7, 1);
    let patch = JournalEntry::Statistics {
        uuid: uuid.to_string(),
        changes: statistics.changes_since(&base.statistics),
        timers: Some(statistics.timers()),
        tick: 3,
    };
    {
        let mut writer = JournalWriter::open(directory.path(), 1).unwrap();
        writer.append(3, 3, std::slice::from_ref(&patch)).unwrap();
        // A complete global batch advances time without any player mutation.
        writer.append(4, 203, &[]).unwrap();
    }
    let mut replay = Replay::default();
    replay.players.insert(uuid.to_string(), base);
    let replay = crate::journal::replay_batches(replay, &[(3, 3, vec![patch.clone(), patch])]);
    assert_eq!(replay.players[&uuid.to_string()].statistics, statistics);
    statistics.freeze_at_tick(203);
    let restarted = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restarted.offline_players[&uuid].statistics, statistics);
    assert_eq!(
        restarted.offline_players[&uuid]
            .statistics
            .get_custom(CustomStat::PlayTime),
        200
    );
    assert_eq!(
        restarted.offline_players[&uuid]
            .statistics
            .get_custom(CustomStat::Jump),
        0
    );
    let mut captured = restarted.offline_players[&uuid].clone();
    captured.tick = 203;
    drop(restarted);
    let checkpoint =
        roundtrip_player(&config, &uuid.to_string(), &captured, &registry.items).unwrap();
    assert_eq!(checkpoint.statistics, statistics);
    let restarted = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    assert_eq!(restarted.offline_players[&uuid].statistics, statistics);
}

#[test]
fn statistics_only_patch_and_non_statistics_full_player_postimage() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::journal::JournalEntry;
    use mc_data::statistics::CustomStat;

    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_path_buf(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    let _store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let (output, _, _) = crate::tick::run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::from_u128(0x1000),
            name: "DurableStats".into(),
            permission_level: 0,
        }],
    );
    crate::core::journal_entries_for_tick(&mut state, &output);
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .statistics
        .increment_custom(CustomStat::Jump, 1);
    let statistics_only = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(statistics_only.iter().any(|entry| matches!(entry,
        JournalEntry::Statistics { changes, .. } if changes.contains(&(8, CustomStat::Jump.id(), 1)))));
    assert!(
        !statistics_only
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Player { .. }))
    );

    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.selected = 1;
    player.statistics.increment_custom(CustomStat::Jump, 1);
    let changed = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(changed.iter().any(|entry| matches!(entry,
        JournalEntry::Player { selected: 1, statistics, .. } if statistics.get_custom(CustomStat::Jump) == 2)));
    assert!(
        !changed
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Statistics { .. }))
    );
}

#[test]
fn idle_statistics_wal_bytes_compare_full_compact_and_lazy_for_ten_players() {
    use crate::boundary::{ConnectionId, PlayerInput};
    use crate::journal::JournalEntry;
    use mc_data::statistics::CustomStat;

    let directory = tempfile::tempdir().unwrap();
    let mut config = CoreConfig::new(directory.path().to_path_buf(), 42);
    config.view_distance = 1;
    config.simulation_distance = 0;
    let _store = fixture_store(&config).unwrap();
    let mut state = crate::core::recover_dimension_state(&config, Dimension::Overworld).unwrap();
    let joins: Vec<_> = (0..10)
        .map(|index| PlayerInput::Join {
            connection: ConnectionId(index + 1),
            uuid: uuid::Uuid::from_u128(0x1000 + u128::from(index)),
            name: format!("Idle{index}"),
            permission_level: 0,
        })
        .collect();
    let (joined, _, _) = crate::tick::run_tick(&mut state, &joins);
    crate::core::journal_entries_for_tick(&mut state, &joined);
    let mut full_bytes = 0_usize;
    let mut compact_bytes = 0_usize;
    let mut lazy_bytes = 0_usize;
    let mut lazy_statistics_bytes = 0_usize;
    for _ in 0..200 {
        let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
        for entry in crate::core::journal_entries_for_tick(&mut state, &output) {
            match &entry {
                JournalEntry::Player { .. } => {
                    lazy_bytes += serde_json::to_vec(&entry).unwrap().len() + 1;
                }
                JournalEntry::Statistics { .. } => {
                    let bytes = serde_json::to_vec(&entry).unwrap().len() + 1;
                    lazy_statistics_bytes += bytes;
                    lazy_bytes += bytes;
                }
                _ => {}
            }
        }
        // Force the previous eager/full-image strategy through the real writer
        // selection path. This test-only baseline is not another live owner.
        state.journaled_players.clear();
        for mut entry in crate::core::journal_entries_for_tick(&mut state, &output) {
            if let JournalEntry::Player {
                uuid,
                statistics,
                tick,
                ..
            } = &mut entry
            {
                let materialized = statistics.at_tick(*tick);
                let changes = [
                    CustomStat::PlayTime,
                    CustomStat::TotalWorldTime,
                    CustomStat::TimeSinceDeath,
                    CustomStat::TimeSinceRest,
                ]
                .map(|stat| (8, stat.id(), materialized.get_custom(stat)))
                .to_vec();
                let compact = JournalEntry::Statistics {
                    uuid: uuid.clone(),
                    changes,
                    timers: None,
                    tick: *tick,
                };
                compact_bytes += serde_json::to_vec(&compact).unwrap().len() + 1;
                **statistics = materialized;
                full_bytes += serde_json::to_vec(&entry).unwrap().len() + 1;
            }
        }
    }
    // 200 ticks = 10 seconds. Extrapolate from 10 players to 1,000.
    let full_1000_bytes_per_second = full_bytes * 10;
    let compact_1000_bytes_per_second = compact_bytes * 10;
    let lazy_1000_bytes_per_second = lazy_bytes * 10;
    eprintln!(
        "idle survival WAL 10 players/200 ticks: full={full_bytes}B compact={compact_bytes}B lazy={lazy_bytes}B \
         (statistics patches={lazy_statistics_bytes}B); \
         1000 players bytes/s: full={full_1000_bytes_per_second} compact={compact_1000_bytes_per_second} lazy={lazy_1000_bytes_per_second}"
    );
    assert_eq!(
        lazy_statistics_bytes, 0,
        "idle statistics timers must not create statistics WAL patches"
    );
    assert_eq!(
        lazy_bytes, 0,
        "idle players must not emit per-tick postimages"
    );
    assert!(
        compact_1000_bytes_per_second > 100_000,
        "compact timer patches exceed the idle WAL budget"
    );
    assert!(full_bytes > compact_bytes);

    // Exercise a zero-valued change without introducing another tick's real
    // warning mutation; the separate regression also checks full postimages.
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    crate::core::journal_entries_for_tick(&mut state, &output);
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .statistics
        .set_custom(CustomStat::Jump, 0);
    let reset = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(reset.iter().any(|entry| matches!(entry,
        JournalEntry::Statistics { changes, .. } if changes.contains(&(8, CustomStat::Jump.id(), 0)))));
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.selected = 1;
    player.statistics.increment_custom(CustomStat::Jump, 1);
    let changed = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(changed.iter().any(|entry| matches!(entry,
        JournalEntry::Player { selected: 1, statistics, .. } if statistics.get_custom(CustomStat::Jump) == 1)));
    assert!(
        !changed
            .iter()
            .any(|entry| matches!(entry, JournalEntry::Statistics { .. }))
    );
}

#[test]
fn pre_plugin_native_checkpoint_opens_with_empty_authority_and_upgrades_on_checkpoint() {
    let directory = tempfile::tempdir().unwrap();
    let config = CoreConfig::new(directory.path().to_owned(), 42);
    let registry = crate::registry::CoreRegistry::load(None).unwrap();
    let path = config.world_dir.join("native/state.redb");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let store = NativeStore::create(&path, native_identity(&config)).unwrap();
    let mut versions = domain_versions();
    versions.remove("plugins");
    versions.remove("plugin_startup");
    let mut manifest = CheckpointManifest {
        domain_versions: versions.clone(),
        world_spawn: Some([0, 80, 0]),
        ..Default::default()
    };
    for dimension in [Dimension::Nether, Dimension::End] {
        manifest.dimensions.insert(
            dimension,
            Box::new(CheckpointManifest {
                domain_versions: versions.clone(),
                world_spawn: Some([0, 80, 0]),
                ..Default::default()
            }),
        );
    }
    let mut globals = generation_records(&config).unwrap();
    globals.insert(
        "checkpoint".into(),
        record(0, serde_json::to_vec(&manifest).unwrap()),
    );
    globals.insert(
        "scoreboard".into(),
        tag_record(0, &crate::scoreboard::Scoreboard::default().to_nbt()).unwrap(),
    );
    for name in ["level_metadata", "conversion_extras"] {
        globals.insert(
            name.into(),
            tag_record(0, &Tag::Compound(Vec::new())).unwrap(),
        );
    }
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 0,
                previous_tick: 0,
                chunks: &[],
                entities: &[],
                players: &[],
                globals: &globals,
            },
            &registry.blocks,
            &registry.items,
        )
        .unwrap();
    drop(store);
    let core = crate::Core::open(config.clone()).unwrap();
    core.shutdown();
    drop(core);
    let upgraded = fixture_store(&config).unwrap().read_checkpoint().unwrap();
    let plugins: crate::plugin::PluginState =
        serde_json::from_slice(&upgraded.globals["plugins"].payload).unwrap();
    assert_eq!(plugins, crate::plugin::PluginState::default());
    let manifest: CheckpointManifest =
        serde_json::from_slice(&upgraded.globals["checkpoint"].payload).unwrap();
    for domain in ["plugins", "plugin_startup"] {
        assert_eq!(manifest.domain_versions.get(domain), Some(&1));
        assert!(upgraded.globals.contains_key(domain));
    }
}
