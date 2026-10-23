use super::*;
use mc_nbt::Tag;
use serde_json::{Value, json};
use std::collections::BTreeMap;

pub(super) fn globals(tick: u64) -> BTreeMap<String, NativeDomainRecord> {
    let manifest = json!({"tick":tick,"dimensions":{},"end_fight":null,"raids":null,"tickets":{},"players":{},"world_spawn":[0,64,0],"world_spawn_rotation":[0,0],"generated_entity_claims":[],"climate":{},"border":{},"difficulty":"Normal","block_ticks":{"next_sequence":0,"chunks":[]},"fluid_ticks":{"next_sequence":0,"covered":[],"due":[]},"gamerules":{},"domain_versions":{"climate":1,"gamerules":1,"block_ticks":1,"fluid_ticks":1,"claims":1,"tickets":1,"raids":1,"end_fight":1}});
    let mut manifest = manifest;
    manifest["domain_versions"]["plugins"] = json!(1);
    manifest["domain_versions"]["plugin_startup"] = json!(1);
    let child = manifest.clone();
    manifest["dimensions"] =
        json!({"minecraft:the_nether":child.clone(),"minecraft:the_end":child});
    let dimensions = json!({"0":{"name":"minecraft:overworld","min_y":-64,"height":384,"generator":"test","seed":1,"revision":1},"1":{"name":"minecraft:the_nether","min_y":0,"height":256,"generator":"test","seed":1,"revision":1},"2":{"name":"minecraft:the_end","min_y":0,"height":256,"generator":"test","seed":1,"revision":1}});
    let generation = json!({"identity":vec![1;32],"base":"fixture","generators":{}});
    let mut records = BTreeMap::new();
    for (key, value) in [
        ("checkpoint", manifest),
        ("dimensions", dimensions),
        ("generation", generation),
        (
            "plugins",
            json!({"storage":{},"receipts":{},"domains":{},"revision":0}),
        ),
        ("plugin_startup", json!({})),
    ] {
        records.insert(
            key.into(),
            NativeDomainRecord {
                version: 1,
                captured_tick: tick,
                payload: serde_json::to_vec(&value).unwrap(),
            },
        );
    }
    for key in [
        "scoreboard",
        "bossbars",
        "level_metadata",
        "conversion_extras",
    ] {
        records.insert(
            key.into(),
            NativeDomainRecord {
                version: 1,
                captured_tick: tick,
                payload: codec::encode(&Tag::Compound(vec![])).unwrap(),
            },
        );
    }
    records
}
pub(super) fn registries() -> (BlockRegistry, mc_data::items::ItemRegistry) {
    (
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
        mc_data::items::solaris_required_items(),
    )
}
fn commit(
    store: &NativeStore,
    tick: u64,
    prev: u64,
    records: &BTreeMap<String, NativeDomainRecord>,
    entities: &[NativeEntitySnapshot],
    players: &[NativePlayerSnapshot],
) -> Result<(), NativeError> {
    let (blocks, items) = registries();
    store.commit_checkpoint(
        NativeCheckpoint {
            tick,
            previous_tick: prev,
            chunks: &[],
            entities,
            players,
            globals: records,
        },
        &blocks,
        &items,
    )
}

#[test]
fn native_checkpoint_zero_versions_and_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    commit(&store, 0, 0, &globals(0), &[], &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    assert!(commit(&store, 0, 0, &globals(0), &[], &[]).is_err());
    let mut records = globals(1);
    records.get_mut("scoreboard").unwrap().version = 2;
    assert!(commit(&store, 1, 0, &records, &[], &[]).is_err());
    let entity = NativeEntitySnapshot {
        identity: [3; 16],
        dimension: 0,
        position: ChunkPos { x: 0, z: 0 },
        generated_slot: Some([4; 32]),
        journal_lsn: 1,
        payload: Some(Tag::Compound(vec![])),
    };
    let bad = NativePlayerSnapshot {
        uuid: [5; 16],
        record: NativeDomainRecord {
            version: 1,
            captured_tick: 1,
            payload: vec![255],
        },
    };
    assert!(commit(&store, 1, 0, &globals(1), &[entity], &[bad]).is_err());
    assert!(store.entity([3; 16]).unwrap().is_none());
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
}

#[test]
fn native_checkpoint_stream_encoding_error_rolls_back_all_domains_and_indexes() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    commit(&store, 0, 0, &globals(0), &[], &[]).unwrap();
    let (blocks, items) = registries();
    let records = globals(1);
    let image = || NativeCheckpoint {
        tick: 1,
        previous_tick: 0,
        chunks: &[],
        entities: &[],
        players: &[],
        globals: &records,
    };
    let entity = NativeEntitySnapshot {
        identity: [3; 16],
        dimension: 0,
        position: ChunkPos { x: 1, z: -2 },
        generated_slot: Some([4; 32]),
        journal_lsn: 1,
        payload: Some(Tag::Compound(vec![("Health".into(), Tag::Float(17.0))])),
    };
    let failed = [
        Ok(&entity),
        Err(NativeError::Invalid("injected entity encoding error")),
    ];
    assert!(
        store
            .commit_checkpoint_iter(image(), failed, &blocks, &items)
            .is_err()
    );
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    assert!(store.entity(entity.identity).unwrap().is_none());
    assert!(
        store
            .generated_slot(entity.generated_slot.unwrap())
            .unwrap()
            .is_none()
    );
    store
        .commit_checkpoint_iter(image(), [Ok(&entity)], &blocks, &items)
        .unwrap();
    let restored = store.read_checkpoint().unwrap();
    assert_eq!(restored.tick, 1);
    assert_eq!(restored.entities.len(), 1);
    assert_eq!(restored.entities[0].payload, entity.payload);
    let reader = store.checkpoint_reader().unwrap();
    let metadata = reader.read_checkpoint_metadata().unwrap();
    assert_eq!(metadata.tick, 1);
    assert!(metadata.entities.is_empty());
    let mut visited = Vec::new();
    reader
        .visit_entities(|snapshot| {
            assert_eq!(snapshot.payload, entity.payload);
            visited.push(snapshot.identity);
            Ok(())
        })
        .unwrap();
    assert_eq!(visited, [entity.identity]);
}
#[test]
fn native_checkpoint_accepts_pre_plugin_manifests_without_plugin_records() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let mut legacy = globals(0);
    legacy.remove("plugins");
    legacy.remove("plugin_startup");
    let record = legacy.get_mut("checkpoint").unwrap();
    let mut manifest: Value = serde_json::from_slice(&record.payload).unwrap();
    for name in ["plugins", "plugin_startup"] {
        manifest["domain_versions"]
            .as_object_mut()
            .unwrap()
            .remove(name);
        for child in manifest["dimensions"].as_object_mut().unwrap().values_mut() {
            child["domain_versions"]
                .as_object_mut()
                .unwrap()
                .remove(name);
        }
    }
    record.payload = serde_json::to_vec(&manifest).unwrap();
    commit(&store, 0, 0, &legacy, &[], &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().globals, legacy);
    commit(&store, 1, 0, &globals(1), &[], &[]).unwrap();
    let upgraded = store.read_checkpoint().unwrap();
    assert!(upgraded.globals.contains_key("plugins"));
    assert!(upgraded.globals.contains_key("plugin_startup"));
}

#[test]
fn native_checkpoint_settlement_rows_are_canonical_or_rejected() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let row = |tick: u64| NativeDomainRecord {
        version: 1,
        captured_tick: tick,
        payload: serde_json::to_vec(&serde_json::json!({
            "id": 7u64,
            "name": "town",
            "owner": "owner",
            "anchor": [0, 64, 0],
            "radius": 48u32,
            "revision": 1u64,
        }))
        .unwrap(),
    };
    let mut canonical = globals(1);
    canonical.insert("settlement/7".into(), row(1));
    canonical.insert(
        "settlement_sequence".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 1,
            payload: serde_json::to_vec(&7u64).unwrap(),
        },
    );
    commit(&store, 1, 0, &canonical, &[], &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().tick, 1);
    for key in [
        "settlement/0",
        "settlement/007",
        "settlement/",
        "settlement/abc",
        "settlement/7/extra",
        "unknown-domain",
    ] {
        let mut bad = globals(2);
        bad.insert(key.into(), row(2));
        bad.insert(
            "settlement_sequence".into(),
            NativeDomainRecord {
                version: 1,
                captured_tick: 2,
                payload: serde_json::to_vec(&7u64).unwrap(),
            },
        );
        assert!(commit(&store, 2, 1, &bad, &[], &[]).is_err(), "{key}");
    }
    let mut stale = globals(2);
    stale.insert("settlement/9".into(), row(1));
    stale.insert(
        "settlement_sequence".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 2,
            payload: serde_json::to_vec(&7u64).unwrap(),
        },
    );
    assert!(commit(&store, 2, 1, &stale, &[], &[]).is_err());
    let mut oversize = globals(2);
    oversize.insert(
        "settlement/9".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 2,
            payload: vec![0; 16 * 1024 * 1024 + 1],
        },
    );
    oversize.insert(
        "settlement_sequence".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 2,
            payload: serde_json::to_vec(&7u64).unwrap(),
        },
    );
    assert!(commit(&store, 2, 1, &oversize, &[], &[]).is_err());
    assert_eq!(store.read_checkpoint().unwrap().tick, 1);
}

#[test]
fn native_checkpoint_settlement_record_plus_sequence_roundtrip() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let mut records = globals(1);
    records.insert(
        "settlement/7".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 1,
            payload: serde_json::to_vec(&serde_json::json!({"dimension": "minecraft:overworld", "value": {"id": 7u64, "name": "town", "owner": "owner", "anchor": [0, 64, 0], "radius": 48u32, "revision": 1u64}})).unwrap(),
        },
    );
    records.insert(
        "settlement_sequence".into(),
        NativeDomainRecord {
            version: 1,
            captured_tick: 1,
            payload: serde_json::to_vec(&7u64).unwrap(),
        },
    );
    commit(&store, 1, 0, &records, &[], &[]).unwrap();
    let image = store.read_checkpoint().unwrap();
    assert_eq!(image.tick, 1);
    assert!(image.globals.contains_key("settlement/7"));
    assert!(image.globals.contains_key("settlement_sequence"));
    // Pre-S1 checkpoint with neither row still commits and reads on a store
    // that never held settlement rows (backward compatible).
    let legacy_dir = tempfile::tempdir().unwrap();
    let legacy = NativeStore::create(&legacy_dir.path().join("state.redb"), [1; 32]).unwrap();
    commit(&legacy, 0, 0, &globals(0), &[], &[]).unwrap();
    commit(&legacy, 2, 0, &globals(2), &[], &[]).unwrap();
    assert_eq!(legacy.read_checkpoint().unwrap().tick, 2);
}

#[test]
fn native_checkpoint_map_rows_and_sequence_roundtrip_or_fail_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let record = |tick, value: serde_json::Value| NativeDomainRecord {
        version: 1,
        captured_tick: tick,
        payload: serde_json::to_vec(&value).unwrap(),
    };
    // Fresh startup checkpoints the allocation fence before any map exists.
    let mut empty = globals(0);
    empty.insert("map_sequence".into(), record(0, json!(0)));
    commit(&store, 0, 0, &empty, &[], &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().globals, empty);
    let mut maps = globals(1);
    maps.insert("map/0".into(), record(1, json!({"colors": []})));
    maps.insert("map_sequence".into(), record(1, json!(1)));
    commit(&store, 1, 0, &maps, &[], &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().globals, maps);

    for (key, sequence, row) in [
        ("map/-1", Some(json!(1)), record(2, json!({}))),
        ("map/00", Some(json!(1)), record(2, json!({}))),
        ("map/+0", Some(json!(1)), record(2, json!({}))),
        ("map/2147483648", Some(json!(1)), record(2, json!({}))),
        ("map/0", None, record(2, json!({}))),
        ("map/0", Some(json!(0)), record(2, json!({}))),
        ("map/0", Some(json!(2147483649_i64)), record(2, json!({}))),
        ("map/0", Some(json!(1)), record(1, json!({}))),
        ("map/0", Some(json!(1)), record(2, json!(null))),
    ] {
        let mut bad = globals(2);
        bad.insert(key.into(), row);
        if let Some(sequence) = sequence {
            bad.insert("map_sequence".into(), record(2, sequence));
        }
        assert!(commit(&store, 2, 1, &bad, &[], &[]).is_err(), "{key}");
        assert_eq!(store.read_checkpoint().unwrap().globals, maps);
    }
}

#[test]
fn native_checkpoint_requires_valid_plugin_domains_without_partial_commit() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let baseline = globals(0);
    commit(&store, 0, 0, &baseline, &[], &[]).unwrap();
    for domain in ["plugins", "plugin_startup"] {
        let mut missing = globals(1);
        missing.remove(domain);
        assert!(commit(&store, 1, 0, &missing, &[], &[]).is_err());
        let mut stale = globals(1);
        stale.get_mut(domain).unwrap().captured_tick = 0;
        assert!(commit(&store, 1, 0, &stale, &[], &[]).is_err());
        let mut malformed = globals(1);
        malformed.get_mut(domain).unwrap().payload = b"null".to_vec();
        assert!(commit(&store, 1, 0, &malformed, &[], &[]).is_err());
        let retained = store.read_checkpoint().unwrap();
        assert_eq!(retained.tick, 0);
        assert_eq!(retained.globals, baseline);
    }
}

#[test]
fn native_checkpoint_unbounded_membership_same_tick_and_stale_replay() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let entities: Vec<_> = (0u32..300)
        .map(|id| {
            let mut identity = [0; 16];
            identity[..4].copy_from_slice(&id.to_be_bytes());
            NativeEntitySnapshot {
                identity,
                dimension: id % 3,
                position: ChunkPos { x: 0, z: 0 },
                generated_slot: None,
                journal_lsn: 1,
                payload: Some(Tag::Compound(vec![("motion".into(), Tag::Int(1))])),
            }
        })
        .collect();
    commit(&store, 1, 0, &globals(1), &entities, &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().entities.len(), 300);
    let mut changed = entities.clone();
    changed[0].payload = Some(Tag::Compound(vec![("motion".into(), Tag::Int(2))]));
    commit(&store, 1, 1, &globals(1), &changed, &[]).unwrap();
    let (blocks, items) = registries();
    assert_eq!(
        store
            .apply_batch(&[], &entities[..1], &blocks, &items)
            .unwrap(),
        0
    );
    assert_eq!(
        store.entity(entities[0].identity).unwrap().unwrap().payload,
        changed[0].payload
    );
    commit(&store, 2, 1, &globals(2), &changed[..1], &[]).unwrap();
    assert!(
        store
            .entity(entities[1].identity)
            .unwrap()
            .unwrap()
            .payload
            .is_none()
    );
    assert!(commit(&store, 3, 2, &globals(3), &entities, &[]).is_err());
}
#[test]
fn native_checkpoint_offline_players_and_pinned_reads() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    let player = NativePlayerSnapshot {
        uuid: [6; 16],
        record: NativeDomainRecord {
            version: 1,
            captured_tick: 1,
            payload: codec::encode(&Tag::Compound(vec![])).unwrap(),
        },
    };
    let mut records = globals(1);
    let mut manifest: serde_json::Value =
        serde_json::from_slice(&records["checkpoint"].payload).unwrap();
    manifest["players"] = json!({uuid::Uuid::from_bytes(player.uuid).to_string():1});
    records.get_mut("checkpoint").unwrap().payload = serde_json::to_vec(&manifest).unwrap();
    commit(&store, 1, 0, &records, &[], std::slice::from_ref(&player)).unwrap();
    let reader = store.checkpoint_reader().unwrap();
    let next = globals(2);
    // The caller supplies only current captures; storage retains offline UUIDs.
    commit(&store, 2, 1, &next, &[], &[]).unwrap();
    assert_eq!(reader.read_checkpoint().unwrap().tick, 1);
    assert_eq!(store.read_checkpoint().unwrap().players, vec![player]);
    drop(reader);
    drop(store);
    let readonly = NativeReadOnlyStore::open_with_cache(&path, [1; 32], 1024 * 1024).unwrap();
    assert_eq!(readonly.read_checkpoint().unwrap().tick, 2);
}

#[test]
fn native_checkpoint_chunk_stream_late_error_rolls_back_and_retries() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    commit(&store, 0, 0, &globals(0), &[], &[]).unwrap();
    let records = globals(1);
    let image = || NativeCheckpoint {
        tick: 1,
        previous_tick: 0,
        chunks: &[],
        entities: &[],
        players: &[],
        globals: &records,
    };
    let visit = |write: &mut dyn FnMut(NativeChunkSnapshot<'_>) -> Result<(), NativeError>| {
        for x in 0..70 {
            // Each pair belongs to this iteration only, not to a world-sized
            // baseline/effective vector or a borrowed snapshot accumulator.
            let chunk = Chunk::empty_with_geometry(
                ChunkPos { x, z: 0 },
                air,
                mc_data::Identifier::parse("minecraft:plains").unwrap(),
                crate::OVERWORLD_GEOMETRY,
            );
            write(NativeChunkSnapshot {
                dimension: 0,
                base: &chunk,
                effective: &chunk,
                dirty_sections: None,
            })?;
        }
        Ok(())
    };
    let failed = store.commit_checkpoint_stream(
        image(),
        |write| {
            visit(write)?;
            Err(NativeError::Source("late chunk preparation failure".into()))
        },
        std::iter::empty::<Result<NativeEntitySnapshot, NativeError>>(),
        &blocks,
        &items,
    );
    assert!(
        failed
            .unwrap_err()
            .to_string()
            .contains("late chunk preparation")
    );
    assert_eq!(store.read_checkpoint().unwrap().tick, 0);
    for x in [0, 69] {
        assert!(
            store
                .resolve_base(
                    0,
                    ChunkPos { x, z: 0 },
                    &blocks,
                    &items,
                    &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                    || Ok(None),
                )
                .unwrap()
                .is_none()
        );
    }
    let io = store
        .commit_checkpoint_stream(
            image(),
            visit,
            std::iter::empty::<Result<NativeEntitySnapshot, NativeError>>(),
            &blocks,
            &items,
        )
        .unwrap();
    assert_eq!(io.rewritten_chunks, 70);
    let stored_chunk_bytes = {
        let tx = store.database.begin_read().unwrap();
        [IMPORTED, METADATA, OVERLAYS]
            .into_iter()
            .map(|name| {
                let table = tx.open_table(name).unwrap();
                table
                    .iter()
                    .unwrap()
                    .map(|row| row.unwrap().1.value().len() as u64)
                    .sum::<u64>()
            })
            .sum::<u64>()
    };
    assert_eq!(io.chunk_payload_bytes, stored_chunk_bytes);
    assert_eq!(io.rewritten_entities, 0);
    assert_eq!(io.entity_payload_bytes, 0);
    drop(store);
    let reopened = NativeStore::open(&path, [1; 32]).unwrap();
    assert_eq!(reopened.read_checkpoint().unwrap().tick, 1);
    for x in [0, 69] {
        assert!(
            reopened
                .resolve_base(
                    0,
                    ChunkPos { x, z: 0 },
                    &blocks,
                    &items,
                    &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                    || Ok(None),
                )
                .unwrap()
                .is_some()
        );
    }
}

struct NoGeneration;
impl crate::ChunkGenerator for NoGeneration {
    fn generate(&self, _: ChunkPos) -> Chunk {
        panic!("imported coverage must win");
    }
}

#[test]
fn native_checkpoint_three_geometries_absolute_imported_ticks_and_fill_guard() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    commit(&store, 0, 0, &globals(0), &[], &[]).unwrap();
    for dimension in 0..3 {
        let geometry = if dimension == 0 {
            crate::OVERWORLD_GEOMETRY
        } else {
            crate::ChunkGeometry::new(0, 256).unwrap()
        };
        let mut chunk = Chunk::empty_with_geometry(
            ChunkPos { x: -1, z: 2 },
            air,
            mc_data::Identifier::parse("minecraft:plains").unwrap(),
            geometry,
        );
        chunk.schedule_block_tick(crate::ScheduledBlockTick::new(
            crate::BlockPos {
                x: -15,
                y: 64,
                z: 33,
            },
            mc_data::Identifier::parse("minecraft:stone").unwrap(),
            u64::MAX - 3,
            -1,
        ));
        chunk.boundary_section_lights[0].block = Some(crate::chunk::LightSection::uniform(0x37));
        store
            .import_chunk(dimension, &chunk, &blocks, &items)
            .unwrap();
        let mut replacement = chunk.clone();
        replacement
            .set_block(
                1,
                64,
                1,
                blocks
                    .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
                    .unwrap()
                    .default,
            )
            .unwrap();
        store
            .fill_generated(dimension, &[replacement], &blocks, &items)
            .unwrap();
        let restored = store
            .read_chunk(
                dimension,
                chunk.pos,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration,
            )
            .unwrap()
            .unwrap();
        assert_eq!(restored.geometry(), geometry);
        assert_eq!(restored.get_block(1, 64, 1), Some(air));
        assert_eq!(
            restored.scheduled_block_ticks(),
            chunk.scheduled_block_ticks()
        );
        assert_eq!(
            restored.boundary_section_lights,
            chunk.boundary_section_lights
        );
    }
    let mut positions = Vec::new();
    store
        .visit_chunk_positions(|dimension, pos| {
            positions.push((dimension, pos));
            Ok(())
        })
        .unwrap();
    assert_eq!(positions.len(), 3);
    assert!(
        store
            .read_chunk(
                0,
                ChunkPos { x: 100, z: 100 },
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration
            )
            .unwrap()
            .is_none()
    );
    drop(store);
    let reopened = NativeStore::open(&path, [1; 32]).unwrap();
    assert!(
        reopened
            .read_chunk(
                2,
                ChunkPos { x: -1, z: 2 },
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration,
            )
            .unwrap()
            .is_some()
    );
}

#[test]
fn native_checkpoint_world_larger_than_64_mib() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let entities: Vec<_> = (0u8..5)
        .map(|id| NativeEntitySnapshot {
            identity: [id; 16],
            dimension: 0,
            position: ChunkPos { x: 0, z: 0 },
            generated_slot: None,
            journal_lsn: 1,
            payload: Some(Tag::Compound(vec![(
                "opaque".into(),
                Tag::ByteArray(vec![id as i8; 14 * 1024 * 1024]),
            )])),
        })
        .collect();
    commit(&store, 1, 0, &globals(1), &entities, &[]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().entities.len(), 5);
}
#[test]
fn native_noop_checkpoint_keeps_stored_chunk_and_entity_bytes() {
    use redb::ReadableDatabase;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let base = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        air,
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let mut effective = base.clone();
    effective.set_block(1, 64, 1, stone).unwrap();
    let entity = NativeEntitySnapshot {
        identity: [7; 16],
        dimension: 0,
        position: ChunkPos { x: 0, z: 0 },
        generated_slot: None,
        journal_lsn: 5,
        payload: Some(Tag::Compound(vec![("health".into(), Tag::Int(20))])),
    };
    // Local raw-byte snapshot: chunk metadata row, entity row, applied LSN,
    // plus a logical-bytes check. No new prod API, no tuple type in prod.
    let raw_bytes = || {
        let logical = store.logical_data_bytes().unwrap();
        let tx = store.database.begin_read().unwrap();
        let chunk_table = tx
            .open_table(redb::TableDefinition::<&[u8], &[u8]>::new("chunk_metadata"))
            .unwrap();
        let key = crate::native::chunk_key(0, ChunkPos { x: 0, z: 0 });
        let chunk = chunk_table
            .get(key.as_slice())
            .unwrap()
            .map(|v| v.value().to_vec());
        drop(chunk_table);
        let entity_table = tx
            .open_table(redb::TableDefinition::<&[u8], &[u8]>::new(
                "persistent_entities",
            ))
            .unwrap();
        let entity_bytes = entity_table
            .get([7; 16].as_slice())
            .unwrap()
            .map(|v| v.value().to_vec());
        drop(entity_table);
        let applied_table = tx
            .open_table(redb::TableDefinition::<&[u8], u64>::new("applied_lsn"))
            .unwrap();
        let lsn = applied_table
            .get(key.as_slice())
            .unwrap()
            .map(|v| v.value());
        drop(applied_table);
        drop(tx);
        (chunk, entity_bytes, lsn, logical)
    };
    let snapshots = [NativeChunkSnapshot {
        dimension: 0,
        base: &base,
        effective: &effective,
        dirty_sections: None,
    }];
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 5,
                previous_tick: 0,
                chunks: &snapshots,
                entities: std::slice::from_ref(&entity),
                players: &[],
                globals: &globals(5),
            },
            &blocks,
            &items,
        )
        .unwrap();
    let before = raw_bytes();
    // Same content at a later global tick: stored bytes must be retained.
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 9,
                previous_tick: 5,
                chunks: &snapshots,
                entities: std::slice::from_ref(&entity),
                players: &[],
                globals: &globals(9),
            },
            &blocks,
            &items,
        )
        .unwrap();
    assert_eq!(raw_bytes(), before);
    // Stale replay below the admitted checkpoint with different content
    // cannot overwrite the retained bytes.
    let mut stale = effective.clone();
    stale.set_block(3, 64, 3, stone).unwrap();
    stale.set_world_journal_lsn(7);
    let mut stale_entity = entity.clone();
    stale_entity.payload = Some(Tag::Compound(vec![("health".into(), Tag::Int(1))]));
    stale_entity.journal_lsn = 7;
    // The global checkpoint covers these older decisions even though the
    // unchanged records retained their earlier per-record stamps.
    assert_eq!(
        store
            .apply_batch(
                &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &base,
                    effective: &stale,
                    dirty_sections: None,
                }],
                &[],
                &blocks,
                &items,
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .apply_batch(&[], std::slice::from_ref(&stale_entity), &blocks, &items)
            .unwrap(),
        0
    );
    assert_eq!(raw_bytes(), before);
    let before_counts = store.record_counts().unwrap();
    let mut unknown_base = base.clone();
    unknown_base.pos = ChunkPos { x: 1, z: 0 };
    let mut unknown_chunk = stale.clone();
    unknown_chunk.pos = unknown_base.pos;
    let mut unknown_entity = stale_entity.clone();
    unknown_entity.identity = [8; 16];
    assert_eq!(
        store
            .apply_batch(
                &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &unknown_base,
                    effective: &unknown_chunk,
                    dirty_sections: None,
                }],
                &[unknown_entity],
                &blocks,
                &items,
            )
            .unwrap(),
        0,
        "checkpoint coverage also fences absent old participants"
    );
    assert_eq!(store.record_counts().unwrap(), before_counts);
    // A real change at a newer tick still writes and survives reopen.
    let mut changed = effective.clone();
    changed.set_block(2, 64, 2, stone).unwrap();
    let changed_snapshots = [NativeChunkSnapshot {
        dimension: 0,
        base: &base,
        effective: &changed,
        dirty_sections: None,
    }];
    let mut changed_entity = entity.clone();
    changed_entity.payload = Some(Tag::Compound(vec![("health".into(), Tag::Int(10))]));
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 10,
                previous_tick: 9,
                chunks: &changed_snapshots,
                entities: std::slice::from_ref(&changed_entity),
                players: &[],
                globals: &globals(10),
            },
            &blocks,
            &items,
        )
        .unwrap();
    drop(store);
    let store = NativeStore::open(&path, [1; 32]).unwrap();
    let restored = store
        .read_chunk(
            0,
            base.pos,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            &NoGeneration,
        )
        .unwrap()
        .unwrap();
    assert_eq!(restored.get_block(2, 64, 2), Some(stone));
    assert_eq!(
        store.entity([7; 16]).unwrap().unwrap().payload,
        changed_entity.payload
    );
    // Omitted entity still tombstones; stale replay cannot undo the checkpoint.
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 11,
                previous_tick: 10,
                chunks: &changed_snapshots,
                entities: &[],
                players: &[],
                globals: &globals(11),
            },
            &blocks,
            &items,
        )
        .unwrap();
    assert!(store.entity([7; 16]).unwrap().unwrap().payload.is_none());
    assert!(commit(&store, 12, 11, &globals(12), &[entity], &[]).is_err());
    let mut offline = changed.clone();
    offline.set_block(4, 64, 4, stone).unwrap();
    assert_eq!(
        store
            .apply_offline_snapshot(0, &base, &mut offline, &blocks, &items)
            .unwrap(),
        12,
        "offline edits advance past the global floor, not a retained row stamp"
    );
}

#[test]
fn native_checkpoint_chunks_roll_back_and_stamp_capture_tick() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let base = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        air,
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let mut effective = base.clone();
    effective.set_block(1, 64, 1, stone).unwrap();
    let snapshots = [NativeChunkSnapshot {
        dimension: 0,
        base: &base,
        effective: &effective,
        dirty_sections: None,
    }];
    let bad = NativePlayerSnapshot {
        uuid: [9; 16],
        record: NativeDomainRecord {
            version: 1,
            captured_tick: 5,
            payload: vec![],
        },
    };
    let records = globals(5);
    assert!(
        store
            .commit_checkpoint(
                NativeCheckpoint {
                    tick: 5,
                    previous_tick: 0,
                    chunks: &snapshots,
                    entities: &[],
                    players: &[bad],
                    globals: &records
                },
                &blocks,
                &items
            )
            .is_err()
    );
    assert_eq!(
        store.record_counts().unwrap(),
        NativeRecordCounts::default()
    );
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 5,
                previous_tick: 0,
                chunks: &snapshots,
                entities: &[],
                players: &[],
                globals: &records,
            },
            &blocks,
            &items,
        )
        .unwrap();
    let restored = store
        .read_chunk(
            0,
            base.pos,
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            &NoGeneration,
        )
        .unwrap()
        .unwrap();
    assert_eq!(restored.world_journal_lsn(), 5);
    assert_eq!(restored.get_block(1, 64, 1), Some(stone));
    assert_eq!(
        store
            .apply_snapshots(&snapshots, &blocks, &items)
            .unwrap_err()
            .to_string(),
        "invalid native record: unstamped snapshot over durable state"
    );
    let mut effective = restored;
    effective.increment_inhabited_time(9);
    let same_tick = [NativeChunkSnapshot {
        dimension: 0,
        base: &base,
        effective: &effective,
        dirty_sections: None,
    }];
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 5,
                previous_tick: 5,
                chunks: &same_tick,
                entities: &[],
                players: &[],
                globals: &records,
            },
            &blocks,
            &items,
        )
        .unwrap();
    assert_eq!(
        store
            .read_chunk(
                0,
                base.pos,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration,
            )
            .unwrap()
            .unwrap()
            .inhabited_time(),
        9
    );
    effective.set_world_journal_lsn(4);
    effective.increment_inhabited_time(20);
    assert_eq!(
        store
            .apply_snapshots(
                &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &base,
                    effective: &effective,
                    dirty_sections: None,
                }],
                &blocks,
                &items
            )
            .unwrap(),
        0
    );
    assert_eq!(
        store
            .read_chunk(
                0,
                base.pos,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration,
            )
            .unwrap()
            .unwrap()
            .inhabited_time(),
        9
    );
}

#[test]
fn native_checkpoint_missing_and_corrupt_domains_fail_closed() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    let store = NativeStore::create(&path, [1; 32]).unwrap();
    let mut missing = globals(1);
    missing.remove("level_metadata");
    assert!(commit(&store, 1, 0, &missing, &[], &[]).is_err());
    let mut trailing = globals(1);
    trailing.get_mut("scoreboard").unwrap().payload.push(0);
    assert!(commit(&store, 1, 0, &trailing, &[], &[]).is_err());
    commit(&store, 1, 0, &globals(1), &[], &[]).unwrap();
    let tx = store.database.begin_write().unwrap();
    {
        let mut header = tx.open_table(HEADER).unwrap();
        header
            .insert("schema", 2u32.to_be_bytes().as_slice())
            .unwrap();
    }
    tx.commit().unwrap();
    drop(store);
    assert!(matches!(
        NativeStore::open(&path, [1; 32]),
        Err(NativeError::UnsupportedSchema(2))
    ));
}

#[test]
fn native_checkpoint_slot_migration_and_wire_conflicts_are_atomic() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let mut entity = NativeEntitySnapshot {
        identity: [2; 16],
        dimension: 0,
        position: ChunkPos { x: 1, z: 1 },
        generated_slot: Some([4; 32]),
        journal_lsn: 1,
        payload: Some(Tag::Compound(vec![("solaris:WireId".into(), Tag::Int(9))])),
    };
    commit(
        &store,
        1,
        0,
        &globals(1),
        std::slice::from_ref(&entity),
        &[],
    )
    .unwrap();
    entity.dimension = 2;
    entity.position = ChunkPos { x: -9, z: 4 };
    entity.journal_lsn = 2;
    commit(
        &store,
        2,
        1,
        &globals(2),
        std::slice::from_ref(&entity),
        &[],
    )
    .unwrap();
    assert_eq!(
        store.generated_slot([4; 32]).unwrap(),
        Some(entity.identity)
    );
    assert_eq!(store.entity(entity.identity).unwrap().unwrap().dimension, 2);
    let conflict = NativeEntitySnapshot {
        identity: [3; 16],
        generated_slot: None,
        ..entity.clone()
    };
    assert!(commit(&store, 3, 2, &globals(3), &[entity.clone(), conflict], &[]).is_err());
    assert_eq!(store.read_checkpoint().unwrap().tick, 2);
    entity.generated_slot = Some([5; 32]);
    assert!(commit(&store, 3, 2, &globals(3), &[entity], &[]).is_err());
    assert!(store.generated_slot([5; 32]).unwrap().is_none());
}

#[test]
fn native_checkpoint_persisted_version_rejection() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    commit(&store, 1, 0, &globals(1), &[], &[]).unwrap();
    let tx = store.database.begin_write().unwrap();
    {
        let definition: TableDefinition<&str, &[u8]> = TableDefinition::new("global");
        let mut table = tx.open_table(definition).unwrap();
        let mut bytes = table.get("scoreboard").unwrap().unwrap().value().to_vec();
        bytes[..4].copy_from_slice(&2u32.to_be_bytes());
        table.insert("scoreboard", bytes.as_slice()).unwrap();
    }
    tx.commit().unwrap();
    assert!(store.read_checkpoint().is_err());
}

#[test]
fn native_checkpoint_staging_dimensions_are_not_a_world() {
    let directory = tempfile::tempdir().unwrap();
    let store = NativeStore::create(&directory.path().join("state.redb"), [1; 32]).unwrap();
    let records = globals(0);
    store.initialize_dimensions(&records["dimensions"]).unwrap();
    assert!(store.read_checkpoint().is_err());
    assert!(store.initialize_dimensions(&records["dimensions"]).is_err());
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let chunk = Chunk::empty_with_geometry(
        ChunkPos { x: 0, z: 0 },
        air,
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
        crate::ChunkGeometry::new(0, 256).unwrap(),
    );
    store.import_chunk(1, &chunk, &blocks, &items).unwrap();
    commit(&store, 0, 0, &records, &[], &[]).unwrap();
    assert!(
        store
            .read_chunk(
                1,
                chunk.pos,
                &blocks,
                &items,
                &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                &NoGeneration,
            )
            .unwrap()
            .is_some()
    );
}

fn commit_checkpoint_then_crash(path: &std::path::Path) -> ! {
    let store = NativeStore::open(path, [1; 32]).unwrap();
    let (blocks, items) = registries();
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    let base = Chunk::empty(
        ChunkPos { x: 2, z: -3 },
        air,
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    let mut effective = base.clone();
    effective.set_block(1, 64, 1, stone).unwrap();
    let entity = NativeEntitySnapshot {
        identity: [2; 16],
        dimension: 0,
        position: base.pos,
        generated_slot: Some([3; 32]),
        journal_lsn: 7,
        payload: Some(Tag::Compound(vec![("solaris:WireId".into(), Tag::Int(9))])),
    };
    store
        .commit_checkpoint(
            NativeCheckpoint {
                tick: 7,
                previous_tick: 0,
                chunks: &[NativeChunkSnapshot {
                    dimension: 0,
                    base: &base,
                    effective: &effective,
                    dirty_sections: None,
                }],
                entities: &[entity],
                players: &[],
                globals: &globals(7),
            },
            &blocks,
            &items,
        )
        .unwrap();
    // Exit immediately after the durable commit, without the clean database drop.
    std::process::exit(39)
}

fn crash_checkpoint_child(path: &std::path::Path, test: &str, environment: &str) {
    let store = NativeStore::create(path, [1; 32]).unwrap();
    commit(&store, 0, 0, &globals(0), &[], &[]).unwrap();
    drop(store);
    let status = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", test, "--nocapture"])
        .env(environment, path)
        .status()
        .unwrap();
    assert_eq!(status.code(), Some(39));
}

#[test]
fn native_checkpoint_writer_reopen_after_crash_keeps_last_immediate_commit() {
    const CHILD: &str = "SOLARIS_NATIVE_A_WRITER_CRASH_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        commit_checkpoint_then_crash(std::path::Path::new(&path));
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    crash_checkpoint_child(
        &path,
        "native::checkpoint_tests::native_checkpoint_writer_reopen_after_crash_keeps_last_immediate_commit",
        CHILD,
    );
    let store = NativeStore::open(&path, [1; 32]).unwrap();
    let checkpoint = store.read_checkpoint().unwrap();
    assert_eq!(checkpoint.tick, 7);
    assert!(
        checkpoint
            .globals
            .values()
            .all(|record| record.captured_tick == 7)
    );
    assert_eq!(checkpoint.entities.len(), 1);
    assert_eq!(checkpoint.entities[0].journal_lsn, 7);
    assert_eq!(store.generated_slot([3; 32]).unwrap(), Some([2; 16]));
    let (blocks, items) = registries();
    let chunk = store
        .read_chunk(
            0,
            ChunkPos { x: 2, z: -3 },
            &blocks,
            &items,
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
            &NoGeneration,
        )
        .unwrap()
        .unwrap();
    let stone = blocks
        .block(&mc_data::Identifier::parse("minecraft:stone").unwrap())
        .unwrap()
        .default;
    assert_eq!(chunk.get_block(1, 64, 1), Some(stone));
}

#[test]
fn native_checkpoint_readonly_crash_refuses_export_without_repairing_source() {
    const CHILD: &str = "SOLARIS_NATIVE_A_READONLY_CRASH_CHILD";
    if let Some(path) = std::env::var_os(CHILD) {
        commit_checkpoint_then_crash(std::path::Path::new(&path));
    }
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("state.redb");
    crash_checkpoint_child(
        &path,
        "native::checkpoint_tests::native_checkpoint_readonly_crash_refuses_export_without_repairing_source",
        CHILD,
    );
    let before = std::fs::read(&path).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let expected = "native source: world was not shut down cleanly; start and stop the server once to repair it, then export";
    let error = NativeReadOnlyStore::open_with_cache(&path, [1; 32], 1024 * 1024)
        .err()
        .expect("read-only open must not repair a crashed writer");
    assert_eq!(error.to_string(), expected);
    assert_eq!(
        NativeStore::inspect_read_only(&path, [1; 32], 1024 * 1024)
            .unwrap_err()
            .to_string(),
        expected
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        std::fs::metadata(&path).unwrap().modified().unwrap(),
        modified
    );
    // Starting/stopping the sole writer repairs redb; export can then read it.
    let store = NativeStore::open(&path, [1; 32]).unwrap();
    assert_eq!(store.read_checkpoint().unwrap().tick, 7);
    drop(store);
    let readonly = NativeReadOnlyStore::open_with_cache(&path, [1; 32], 1024 * 1024).unwrap();
    assert_eq!(readonly.read_checkpoint().unwrap().tick, 7);
}
