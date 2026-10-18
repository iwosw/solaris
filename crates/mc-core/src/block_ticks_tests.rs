use std::sync::{Arc, LazyLock};

use super::*;
use crate::journal::{
    CheckpointManifest, JournalEntry, JournalWriter, Replay, read_complete_batches_after,
    replay_batches,
};
use crate::native_checkpoint::tests::{read_manifest, write_manifest};
use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;
use mc_world::{BlockStateId, ChunkPos};

fn config(path: &std::path::Path) -> crate::CoreConfig {
    crate::CoreConfig::new(path.to_owned(), 42)
}

fn pos(x: i32) -> BlockPos {
    BlockPos { x, y: 200, z: 1 }
}

fn tick(x: i32, when: u64, priority: i32) -> ScheduledBlockTick {
    ScheduledBlockTick::new(
        pos(x),
        Identifier::parse("minecraft:stone").unwrap(),
        when,
        priority,
    )
}

fn registry() -> Arc<CoreRegistry> {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> =
        LazyLock::new(|| Arc::new(CoreRegistry::load(None).unwrap()));
    Arc::clone(&REGISTRY)
}

fn world(dir: Option<std::path::PathBuf>) -> ChunkWorld {
    let registry = registry();
    let generator = Arc::new(mc_worldgen::TerrainGenerator::new(
        42,
        Arc::clone(&registry.blocks),
    ));
    if let Some(dir) = dir {
        ChunkWorld::new_native(
            42,
            crate::native_checkpoint::tests::fixture_store(&config(&dir)).unwrap(),
            0,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [0, 201, 2],
            mc_world::OVERWORLD_GEOMETRY,
        )
    } else {
        ChunkWorld::new(
            42,
            generator,
            &registry,
            BlockStateId(registry.states.air),
            [0, 201, 2],
            mc_world::OVERWORLD_GEOMETRY,
        )
    }
}

fn directory() -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("solaris-block-ticks-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn scheduler_fail_closed_cases() -> Vec<(&'static str, CheckpointManifest, bool)> {
    let mut blocks = BlockTicks::default();
    blocks.schedule(tick(1, 10, 0));
    blocks.schedule(tick(17, 10, 0));
    let block_image = blocks.image();
    let mut fluids = crate::fluids::FluidTicks::default();
    let water = Identifier::parse("minecraft:water").unwrap();
    fluids.schedule(pos(1), &water, 10, 0);
    fluids.schedule(pos(17), &water, 10, 0);
    let fluid_image = fluids.image();
    let mut cases = Vec::new();
    for case in 0..10 {
        let mut image = block_image.clone();
        let (error, survives_wal_fold) = match case {
            0 => {
                image.next_sequence = u64::MAX;
                image.chunks.clear();
                ("block_ticks: next_sequence", true)
            }
            1 => {
                image.next_sequence = u64::MAX;
                ("block_ticks: next_sequence", true)
            }
            2 => {
                image.next_sequence = 1;
                ("not below next_sequence", true)
            }
            3 => {
                let mut duplicate = image.chunks[0].ticks[0].clone();
                duplicate.pos[0] = 2;
                image.chunks[0].ticks.push(duplicate);
                ("duplicate order", true)
            }
            4 => {
                image.chunks[1].ticks[0].sequence = image.chunks[0].ticks[0].sequence;
                ("duplicate order", true)
            }
            5 => {
                image.chunks[1].ticks[0].sequence = image.chunks[0].ticks[0].sequence;
                image.chunks[1].ticks[0].trigger_tick += 1;
                ("duplicate sequence", true)
            }
            6 => {
                let mut duplicate = image.chunks[0].ticks[0].clone();
                duplicate.sequence = 2;
                image.next_sequence = 3;
                image.chunks[0].ticks.push(duplicate);
                ("duplicate tick identity", true)
            }
            7 => {
                image.chunks[0].ticks[0].pos[0] = 17;
                ("outside chunk coverage", true)
            }
            8 => {
                image.chunks.push(image.chunks[1].clone());
                ("coverage is not strictly ordered", false)
            }
            9 => {
                image.chunks.reverse();
                ("coverage is not strictly ordered", false)
            }
            _ => unreachable!(),
        };
        cases.push((
            error,
            CheckpointManifest {
                block_ticks: image,
                ..Default::default()
            },
            survives_wal_fold,
        ));
    }
    for case in 0..9 {
        let mut image = fluid_image.clone();
        let error = match case {
            0 => {
                image.next_sequence = u64::MAX;
                image.due.clear();
                "fluid_ticks: next_sequence"
            }
            1 => {
                image.next_sequence = u64::MAX;
                "fluid_ticks: next_sequence"
            }
            2 => {
                image.next_sequence = 1;
                "not below next_sequence"
            }
            3 => {
                image.due[1].sequence = image.due[0].sequence;
                "duplicate order"
            }
            4 => {
                image.due[1].sequence = image.due[0].sequence;
                image.due[1].deadline += 1;
                "duplicate sequence"
            }
            5 => {
                let mut duplicate = image.due[0].clone();
                duplicate.sequence = 2;
                image.next_sequence = 3;
                image.due.push(duplicate);
                "duplicate tick identity"
            }
            6 => {
                image.covered.clear();
                "missing chunk coverage"
            }
            7 => {
                image.covered.push(image.covered[0]);
                "duplicate coverage"
            }
            8 => {
                image.due[0].fluid = Identifier::parse("minecraft:milk").unwrap();
                "unsupported fluid identifier"
            }
            _ => unreachable!(),
        };
        cases.push((
            error,
            CheckpointManifest {
                fluid_ticks: image,
                ..Default::default()
            },
            true,
        ));
    }
    cases
}

fn scheduler_manifest_in_dimension(
    manifest: CheckpointManifest,
    dimension: crate::dimensions::Dimension,
) -> CheckpointManifest {
    if dimension == crate::dimensions::Dimension::Overworld {
        manifest
    } else {
        CheckpointManifest {
            dimensions: [(dimension, Box::new(manifest))].into_iter().collect(),
            ..Default::default()
        }
    }
}

#[test]
fn scheduler_wal_folded_order_fail_closed_against_checkpoint_base() {
    use crate::dimensions::Dimension;
    for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
        let dir = tempfile::tempdir().unwrap();
        let mut queue = BlockTicks::default();
        queue.schedule(tick(1, 10, 0));
        let base = queue.image();
        let manifest = scheduler_manifest_in_dimension(
            CheckpointManifest {
                block_ticks: base.clone(),
                ..Default::default()
            },
            dimension,
        );
        write_manifest(&config(dir.path()), &manifest).unwrap();
        // This row is valid in isolation. Its order collides only after
        // folding the new chunk over the retained checkpoint chunk.
        let mut collision = base.chunks[0].ticks[0].clone();
        collision.pos[0] = 17;
        let image = BlockTickChunkImage {
            chunk: [1, 0],
            ticks: vec![collision],
        };
        BlockTicksImage {
            next_sequence: 2,
            chunks: vec![image.clone()],
        }
        .validate()
        .unwrap();
        let mut entries = vec![JournalEntry::BlockTicks {
            image,
            next_sequence: 2,
        }];
        if dimension != Dimension::Overworld {
            entries = vec![JournalEntry::Dimension { dimension, entries }];
        }
        let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
        writer.append(2, 2, &entries).unwrap();
        writer.flush().unwrap();
        drop(writer);
        let manifest = read_manifest(&config(dir.path())).unwrap();
        let expected_startup = crate::native_checkpoint::recover_offline(&config(dir.path()))
            .err()
            .expect("corrupt folded scheduler must fail admission")
            .to_string();
        assert!(
            expected_startup.contains("duplicate order"),
            "{expected_startup}"
        );
        let error = crate::Core::open(crate::CoreConfig::new(dir.path().to_owned(), 42))
            .unwrap_err()
            .to_string();
        assert_eq!(error, expected_startup);
        assert_eq!(
            read_manifest(&config(dir.path())).unwrap().tick,
            manifest.tick
        );
    }
}

#[test]
fn scheduler_manifest_fail_closed_before_adoption_or_writes() {
    use crate::dimensions::Dimension;
    for (expected, malformed, _) in scheduler_fail_closed_cases() {
        for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
            let dir = tempfile::tempdir().unwrap();
            let manifest = scheduler_manifest_in_dimension(malformed.clone(), dimension);
            write_manifest(&config(dir.path()), &manifest).unwrap();
            let manifest = read_manifest(&config(dir.path())).unwrap();
            let expected_startup = crate::native_checkpoint::validate_manifest(&manifest)
                .unwrap_err()
                .to_string();
            assert!(
                expected_startup.contains(expected),
                "{expected}: {expected_startup}"
            );
            let error = crate::Core::open(crate::CoreConfig::new(dir.path().to_owned(), 42))
                .unwrap_err()
                .to_string();
            assert_eq!(error, expected_startup);
            assert_eq!(
                read_manifest(&config(dir.path())).unwrap().tick,
                manifest.tick
            );
        }
    }
}

#[test]
fn scheduler_wal_final_image_fail_closed_before_adoption_or_writes() {
    use crate::dimensions::Dimension;
    for (expected, malformed, survives_wal_fold) in scheduler_fail_closed_cases() {
        if !survives_wal_fold {
            // Absolute WAL chunk post-images replace coverage by key; duplicate
            // or unordered manifest chunk lists cannot be produced by that fold.
            continue;
        }
        for dimension in [Dimension::Overworld, Dimension::Nether, Dimension::End] {
            let dir = tempfile::tempdir().unwrap();
            write_manifest(&config(dir.path()), &CheckpointManifest::default()).unwrap();
            let mut entries: Vec<_> = malformed
                .block_ticks
                .chunks
                .iter()
                .cloned()
                .map(|image| JournalEntry::BlockTicks {
                    image,
                    next_sequence: malformed.block_ticks.next_sequence,
                })
                .collect();
            if entries.is_empty() && malformed.block_ticks.next_sequence != 0 {
                entries.push(JournalEntry::BlockTicks {
                    image: BlockTickChunkImage {
                        chunk: [0, 0],
                        ticks: Vec::new(),
                    },
                    next_sequence: malformed.block_ticks.next_sequence,
                });
            }
            entries.push(JournalEntry::FluidTicks {
                image: malformed.fluid_ticks.clone(),
            });
            if dimension != Dimension::Overworld {
                entries = vec![JournalEntry::Dimension { dimension, entries }];
            }
            let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
            writer.append(2, 2, &entries).unwrap();
            writer.flush().unwrap();
            drop(writer);
            let manifest = read_manifest(&config(dir.path())).unwrap();
            let error = crate::native_checkpoint::recover_offline(&config(dir.path()))
                .err()
                .expect("corrupt WAL scheduler must fail admission")
                .to_string();
            assert!(error.contains("scheduler recovery"), "{error}");
            assert!(error.contains(expected), "{expected}: {error}");
            if dimension != Dimension::Overworld {
                assert!(error.contains(&format!("{dimension:?}")), "{error}");
            }
            let expected_startup = error;
            let error = crate::Core::open(crate::CoreConfig::new(dir.path().to_owned(), 42))
                .unwrap_err()
                .to_string();
            assert_eq!(error, expected_startup);
            assert_eq!(
                read_manifest(&config(dir.path())).unwrap().tick,
                manifest.tick
            );
        }
    }
}

#[test]
fn scheduler_fail_closed_boundary_preserves_valid_images_and_final_tombstones() {
    use crate::dimensions::Dimension;
    let dir = tempfile::tempdir().unwrap();
    let mut blocks = BlockTicks::default();
    blocks.schedule(tick(1, 10, 0));
    blocks.schedule(tick(17, 10, 0));
    blocks.schedule(tick(1, 11, 0));
    let mut fluids = crate::fluids::FluidTicks::default();
    let water = Identifier::parse("minecraft:water").unwrap();
    fluids.schedule(pos(1), &water, 10, 0);
    fluids.schedule(pos(17), &water, 10, 0);
    fluids.schedule(pos(1), &water, 11, 0);
    let image = CheckpointManifest {
        block_ticks: blocks.image(),
        fluid_ticks: fluids.image(),
        ..Default::default()
    };
    let manifest = CheckpointManifest {
        dimensions: [Dimension::Nether, Dimension::End]
            .into_iter()
            .map(|dimension| (dimension, Box::new(image.clone())))
            .collect(),
        ..image.clone()
    };
    write_manifest(&config(dir.path()), &manifest).unwrap();
    let persisted = read_manifest(&config(dir.path())).unwrap();
    assert_eq!(persisted.block_ticks, manifest.block_ticks);
    assert_eq!(persisted.fluid_ticks, manifest.fluid_ticks);
    for dimension in [Dimension::Nether, Dimension::End] {
        assert_eq!(
            persisted.dimensions[&dimension].block_ticks,
            manifest.dimensions[&dimension].block_ticks
        );
        assert_eq!(
            persisted.dimensions[&dimension].fluid_ticks,
            manifest.dimensions[&dimension].fluid_ticks
        );
    }
    crate::native_checkpoint::recover_offline(&config(dir.path())).unwrap();

    let mut restored = BlockTicks::from_image(image.block_ticks, None);
    assert!(restored.contains(pos(1), &Identifier::parse("minecraft:stone").unwrap(), 0));
    assert!(!restored.schedule(tick(1, 10, 0)));
    assert!(restored.schedule(tick(33, 10, 0)));
    assert_eq!(restored.drain_ready(10, 10, |_| true).len(), 3);
    restored.image().validate().unwrap();
    let mut restored_fluid = crate::fluids::FluidTicks::from_image(image.fluid_ticks, None);
    assert!(!restored_fluid.schedule(pos(1), &water, 10, 0));
    assert!(restored_fluid.schedule(pos(33), &water, 10, 0));
    restored_fluid.image().validate().unwrap();

    let cleared_fluid = crate::fluids::FluidTicksImage {
        due: Vec::new(),
        covered: fluids.image().covered,
        next_sequence: fluids.image().next_sequence,
    };
    let mut entries = vec![JournalEntry::FluidTicks {
        image: cleared_fluid.clone(),
    }];
    for chunk in &blocks.image().chunks {
        entries.push(JournalEntry::BlockTicks {
            image: BlockTickChunkImage {
                chunk: chunk.chunk,
                ticks: Vec::new(),
            },
            next_sequence: blocks.image().next_sequence,
        });
    }
    let all_dimensions: Vec<_> = [Dimension::Overworld, Dimension::Nether, Dimension::End]
        .into_iter()
        .map(|dimension| JournalEntry::Dimension {
            dimension,
            entries: entries.clone(),
        })
        .collect();
    let mut writer = JournalWriter::open(dir.path(), 1).unwrap();
    writer.append(1, 1, &all_dimensions).unwrap();
    writer.flush().unwrap();
    drop(writer);
    let before = read_manifest(&config(dir.path())).unwrap().tick;
    crate::native_checkpoint::recover_offline(&config(dir.path())).unwrap();
    assert_eq!(read_manifest(&config(dir.path())).unwrap().tick, before);
    let replay = replay_batches(
        Replay {
            block_ticks: manifest.block_ticks,
            fluid_ticks: manifest.fluid_ticks,
            dimensions: manifest
                .dimensions
                .into_iter()
                .map(|(dimension, child)| {
                    (
                        dimension,
                        Box::new(Replay {
                            block_ticks: child.block_ticks,
                            fluid_ticks: child.fluid_ticks,
                            ..Default::default()
                        }),
                    )
                })
                .collect(),
            ..Default::default()
        },
        &read_complete_batches_after(&dir.path().join("journal"), 0).unwrap(),
    );
    for recovered in std::iter::once(&replay).chain(replay.dimensions.values().map(Box::as_ref)) {
        assert_eq!(recovered.fluid_ticks, cleared_fluid);
        assert!(
            recovered
                .block_ticks
                .chunks
                .iter()
                .all(|chunk| chunk.ticks.is_empty())
        );
        recovered.block_ticks.validate().unwrap();
        recovered.fluid_ticks.validate().unwrap();
    }
}

#[test]
fn due_order_is_deadline_priority_then_global_insertion_across_chunks() {
    let mut queue = BlockTicks::default();
    assert!(queue.schedule(tick(17, 11, -3)));
    assert!(queue.schedule(tick(-1, 10, 2)));
    assert!(queue.schedule(tick(1, 10, -1)));
    assert!(queue.schedule(tick(33, 10, -1)));
    assert!(queue.drain_ready(9, 10, |_| true).is_empty());
    assert!(queue.drain_ready(10, 0, |_| true).is_empty());
    let first = queue.drain_ready(10, 2, |_| true);
    assert_eq!(
        first.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        [1, 33]
    );
    assert_eq!(
        first
            .iter()
            .map(ScheduledBlockTick::sequence)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(queue.drain_ready(10, 10, |_| true)[0].pos.x, -1);
    assert_eq!(queue.drain_ready(11, 10, |_| true)[0].pos.x, 17);
    assert!(queue.drain_ready(u64::MAX, 10, |_| true).is_empty());
}

#[test]
fn dedup_matches_native_request_identity_and_does_not_consume_sequence() {
    let mut queue = BlockTicks::default();
    assert!(queue.schedule(tick(1, 10, 0)));
    assert!(!queue.schedule(tick(1, 10, 0)));
    assert!(queue.schedule(tick(1, 11, 0)));
    assert!(queue.schedule(tick(1, 10, -1)));
    assert!(queue.schedule(tick(2, 10, 0)));
    assert!(queue.schedule(ScheduledBlockTick::new(
        pos(1),
        Identifier::parse("minecraft:dirt").unwrap(),
        10,
        0,
    )));
    let stone = Identifier::parse("minecraft:stone").unwrap();
    let dirt = Identifier::parse("minecraft:dirt").unwrap();
    assert!(queue.contains(pos(1), &stone, -1));
    assert!(queue.contains(pos(1), &stone, 0));
    assert!(queue.contains(pos(1), &dirt, 0));
    assert!(!queue.contains(pos(1), &dirt, -1));
    assert!(!queue.contains(pos(1), &stone, 1));
    let drained = queue.drain_ready(11, 10, |_| true);
    assert_eq!(
        drained
            .iter()
            .map(ScheduledBlockTick::sequence)
            .collect::<Vec<_>>(),
        [2, 0, 3, 4, 1]
    );
    assert_eq!(queue.image().next_sequence, 5);
    assert!(!queue.contains(pos(1), &stone, 0));
    // A completed identity can be scheduled again with a new insertion ID.
    assert!(queue.schedule(tick(1, 10, 0)));
    assert_eq!(queue.drain_ready(11, 1, |_| true)[0].sequence(), 5);
}

#[test]
fn unavailable_cells_keep_identity_and_do_not_starve_available_due_work() {
    let mut queue = BlockTicks::default();
    queue.schedule(tick(17, 1, -3));
    queue.schedule(tick(1, 2, 0));
    queue.schedule(tick(2, 2, 0));
    let retained = queue.due.values().next().unwrap().clone();
    assert_eq!(queue.drain_ready(20, 1, |pos| pos.x < 16)[0].pos.x, 1);
    assert_eq!(queue.due.values().next().unwrap(), &retained);
    assert_eq!(queue.drain_ready(20, 1, |pos| pos.x < 16)[0].pos.x, 2);
    assert!(queue.drain_ready(20, 1, |_| false).is_empty());
    assert_eq!(queue.drain_ready(20, 1, |_| true), [retained]);
}

#[test]
fn absolute_checkpoint_image_preserves_fifo_and_clock_across_restart() {
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 1020, 0));
    queue.schedule(tick(17, 1020, 0));
    let bytes = serde_json::to_vec(&queue.image()).unwrap();
    let mut restored = BlockTicks::from_image(serde_json::from_slice(&bytes).unwrap(), None);
    assert!(!restored.schedule(tick(1, 1020, 0)));
    assert!(restored.schedule(tick(-1, 1020, 0)));
    assert!(restored.drain_ready(1019, 10, |_| true).is_empty());
    let drained = restored.drain_ready(1020, 10, |_| true);
    assert_eq!(
        drained.iter().map(|tick| tick.pos.x).collect::<Vec<_>>(),
        [1, 17, -1]
    );
    assert_eq!(
        drained
            .iter()
            .map(ScheduledBlockTick::sequence)
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

#[test]
fn vanilla_region_fields_and_imported_delays_are_retained_once() {
    let registry = registry();
    let mut chunk = Chunk::empty(
        ChunkPos { x: 0, z: 0 },
        BlockStateId(registry.states.air),
        Identifier::parse("minecraft:plains").unwrap(),
    );
    assert!(chunk.schedule_block_tick(tick(1, 110, 2)));
    assert!(chunk.schedule_block_tick(tick(2, 110, -1)));
    let payload = mc_world::anvil::chunk_to_payload_with_items_at_tick(
        &chunk,
        &registry.blocks,
        Some(&registry.items),
        0,
        100,
    )
    .unwrap();
    let (_, root) = mc_nbt::read_named(&mut payload.uncompressed_nbt.as_slice()).unwrap();
    let mc_nbt::Tag::Compound(fields) = &root else {
        panic!("chunk compound")
    };
    let mc_nbt::Tag::List(ticks) = &fields
        .iter()
        .find(|(name, _)| name == "block_ticks")
        .unwrap()
        .1
    else {
        panic!("block_ticks list")
    };
    assert_eq!(ticks.elements.len(), 2);
    let mc_nbt::Tag::Compound(record) = &ticks.elements[0] else {
        panic!("tick compound")
    };
    assert_eq!(
        record
            .iter()
            .map(|(key, _)| key.as_str())
            .collect::<Vec<_>>(),
        ["i", "x", "y", "z", "t", "p"]
    );
    assert_eq!(
        record.iter().find(|(name, _)| name == "t").unwrap().1,
        mc_nbt::Tag::Int(10)
    );
    assert_eq!(
        record.iter().find(|(name, _)| name == "p").unwrap().1,
        mc_nbt::Tag::Int(-1)
    );
    let loaded = mc_world::anvil::chunk_from_nbt_with_items(
        &root,
        &registry.blocks,
        Some(&registry.items),
        &registry.chest_loot,
        mc_world::OVERWORLD_GEOMETRY,
    )
    .unwrap();
    let mut queue = BlockTicks::default();
    queue.hydrate_chunk(&loaded, 900);
    queue.hydrate_chunk(&loaded, 950);
    queue.image().validate().unwrap();
    assert!(queue.drain_ready(909, 10, |_| true).is_empty());
    assert_eq!(
        queue
            .drain_ready(910, 10, |_| true)
            .iter()
            .map(|tick| tick.pos.x)
            .collect::<Vec<_>>(),
        [2, 1]
    );
    // The empty absolute image prevents stale region data from resurrecting.
    queue.image().validate().unwrap();
    let mut restored = BlockTicks::from_image(queue.image(), None);
    restored.hydrate_chunk(&loaded, 1000);
    assert!(restored.drain_ready(u64::MAX, 10, |_| true).is_empty());
    assert!(loaded.block_entities.is_empty());
}

#[test]
fn journal_roundtrip_is_idempotent_and_replays_empty_queue_tombstones() {
    let dir = directory();
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 20, 0));
    queue.schedule(tick(17, 30, -1));
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    let base = replay_batches(Replay::default(), &[(1, 1, entries.clone())]);
    assert_eq!(base.block_ticks, queue.image());
    let twice = replay_batches(
        Replay::default(),
        &[(1, 1, entries.clone()), (1, 1, entries.clone())],
    );
    assert_eq!(twice.block_ticks, base.block_ticks);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 1, &entries).unwrap();
    assert_eq!(queue.drain_ready(20, 10, |_| true).len(), 1);
    entries.clear();
    queue.journal_entries(&mut entries);
    assert!(
        matches!(&entries[0], JournalEntry::BlockTicks { image, .. } if image.chunk == [0,0] && image.ticks.is_empty())
    );
    // The second batch covers the idle interval as well as the consumed tick.
    writer.append(2, 20, &entries).unwrap();
    writer.flush().unwrap();
    let batches = read_complete_batches_after(&dir.join("journal"), 0).unwrap();
    assert_eq!(batches.len(), 2);
    let replay = replay_batches(Replay::default(), &batches);
    assert_eq!(replay.tick, 20);
    assert_eq!(replay.block_ticks, queue.image());
    let mut restored = BlockTicks::from_image(replay.block_ticks, None);
    assert!(restored.drain_ready(29, 10, |_| true).is_empty());
    let remaining = restored.drain_ready(30, 10, |_| true);
    assert_eq!(remaining[0].pos, pos(17));
    assert_eq!(remaining[0].sequence(), 1);
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn block_container_and_queue_post_images_share_actual_commit_batch() {
    let registry = registry();
    let mut state = crate::tick::TickState::new(Arc::clone(&registry), world(None), 0, 512);
    let chest = registry
        .blocks
        .block(&Identifier::parse("minecraft:chest").unwrap())
        .unwrap()
        .default
        .0;
    state.merge_block_writes.push((pos(1), chest));
    state.block_entities.insert(
        (1, 200, 1),
        crate::tick::BlockEntityState {
            slots: vec![mc_data::ItemStack::EMPTY; 27],
            furnace: None,
        },
    );
    state.block_ticks.schedule(ScheduledBlockTick::new(
        pos(1),
        Identifier::parse("minecraft:chest").unwrap(),
        20,
        0,
    ));
    let (output, _, _) = crate::tick::run_tick(&mut state, &[]);
    let entries = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(entries.iter().any(|entry| matches!(entry, JournalEntry::Block { pos, state } if *pos == [1,200,1] && *state == chest)));
    assert!(
        entries.iter().any(
            |entry| matches!(entry, JournalEntry::BlockEntity { pos, .. } if *pos == [1,200,1])
        )
    );
    assert!(entries.iter().any(|entry| matches!(entry, JournalEntry::BlockTicks { image, .. } if image.chunk == [0,0] && image.ticks.len() == 1)));
    let replay = replay_batches(Replay::default(), &[(output.tick, output.tick, entries)]);
    assert_eq!(replay.blocks[&(1, 200, 1)], chest);
    assert_eq!(replay.block_entities[&(1, 200, 1)].slots.len(), 27);
    assert_eq!(replay.block_ticks.chunks[0].ticks[0].trigger_tick, 20);
    let next = crate::core::journal_entries_for_tick(&mut state, &output);
    assert!(
        !next
            .iter()
            .any(|entry| matches!(entry, JournalEntry::BlockTicks { .. }))
    );
}

#[test]
fn checkpoint_writes_vanilla_remaining_delay_and_restores_absolute_deadline() {
    let dir = directory();
    let cpos = ChunkPos { x: 0, z: 0 };
    let want = [(cpos, 0)].into_iter().collect();
    let mut world = world(Some(dir.clone()));
    world.ensure_blocking(&want);
    let mut queue = BlockTicks::default();
    queue.hydrate_chunk(world.chunk(cpos).unwrap(), 100);
    queue.schedule(tick(1, 120, -1));
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    writer.append(1, 100, &entries).unwrap();
    writer.gate(0, 100).unwrap();
    let absent = queue.checkpoint(&mut world, 100);
    let mut capture = world.capture_dirty(&Default::default());
    world.capture_scheduled_block_ticks(&mut capture, absent);
    let written = crate::world::world_tests::write_capture(&capture).unwrap();
    assert_eq!(written.len(), 1);
    world.mark_flushed(&written);
    write_manifest(
        &config(&dir),
        &CheckpointManifest {
            tick: 100,
            block_ticks: queue.image(),
            ..Default::default()
        },
    )
    .unwrap();
    let manifest = read_manifest(&config(&dir)).unwrap();
    assert_eq!(manifest.tick, 100);
    drop(world);
    drop(writer);
    let mut world = self::world(Some(dir.clone()));
    world.ensure_blocking(&want);
    world.set_active_chunks([cpos].into_iter().collect(), 100);
    let loaded = world.chunk(cpos).unwrap();
    assert_eq!(loaded.scheduled_block_ticks()[0].trigger_tick, 20);
    assert_eq!(loaded.scheduled_block_ticks()[0].priority, -1);
    assert!(loaded.block_entities.is_empty());
    let mut restored = BlockTicks::from_image(manifest.block_ticks, None);
    restored.hydrate_chunk(loaded, 100);
    assert!(restored.drain_loaded(119, 10, &world).is_empty());
    let due = restored.drain_loaded(120, 10, &world);
    assert_eq!(due[0].trigger_tick, 120);
    assert_eq!(due[0].sequence(), 0);
    restored.hydrate_chunk(world.chunk(cpos).unwrap(), 200);
    assert!(restored.drain_loaded(300, 10, &world).is_empty());
    drop(world);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn eviction_and_unloaded_overlay_do_not_consume_due_cells() {
    let dir = directory();
    let mut world = world(Some(dir.clone()));
    let cpos = ChunkPos { x: 0, z: 0 };
    let want = [(cpos, 0)].into_iter().collect();
    world.ensure_blocking(&want);
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 10, 0));
    let identity = queue.due.values().next().unwrap().clone();
    assert_eq!(world.evict_outside(&Default::default()), [cpos]);
    let mut writer = JournalWriter::open(&dir, 1).unwrap();
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    writer.append(1, 5, &entries).unwrap();
    writer.gate(0, 5).unwrap();
    let absent = queue.checkpoint(&mut world, 5);
    assert!(!world.has_chunk(0, 0));
    let mut capture = world.capture_dirty(&Default::default());
    world.capture_scheduled_block_ticks(&mut capture, absent);
    assert_eq!(
        crate::world::world_tests::write_capture(&capture)
            .unwrap()
            .len(),
        1
    );
    assert!(!world.has_chunk(0, 0));
    assert!(queue.drain_loaded(20, 10, &world).is_empty());
    write_manifest(
        &config(&dir),
        &CheckpointManifest {
            tick: 5,
            block_ticks: queue.image(),
            ..Default::default()
        },
    )
    .unwrap();
    // A replay overlay can answer block_at even without a resident chunk.
    world.set_block(1, 200, 1, registry().states.stone);
    assert!(world.block_at(1, 200, 1).is_some());
    assert!(queue.drain_loaded(20, 10, &world).is_empty());
    assert_eq!(queue.due.values().next().unwrap(), &identity);
    world.ensure_blocking(&want);
    world.set_active_chunks([cpos].into_iter().collect(), 20);
    assert_eq!(
        world.chunk(cpos).unwrap().scheduled_block_ticks()[0].trigger_tick,
        5
    );
    queue.hydrate_chunk(world.chunk(cpos).unwrap(), 20);
    assert_eq!(queue.drain_loaded(20, 10, &world), [identity]);
    drop(world);
    drop(writer);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn coverage_eviction_requires_captured_empty_flush_ack_unloaded_and_clean() {
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 10, 0));
    let nonempty_checkpoint = queue.image();
    queue.drain_ready(10, 1, |_| true);
    let empty_checkpoint = queue.image();
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.covered.contains(&(0, 0)),
        "unflushed tombstone stays authoritative"
    );
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.covered.contains(&(0, 0)),
        "WAL emission is not a native checkpoint"
    );
    queue.acknowledge_flushed(&nonempty_checkpoint);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.covered.contains(&(0, 0)),
        "older nonempty checkpoint cannot authorize eviction"
    );
    queue.acknowledge_flushed(&empty_checkpoint);
    queue.evict_flushed(|_, _| false);
    assert!(
        queue.covered.contains(&(0, 0)),
        "loaded or dirty world column stays covered"
    );
    queue.evict_flushed(|_, _| true);
    assert!(!queue.covered.contains(&(0, 0)));
    queue.schedule(tick(1, 20, 0));
    queue.acknowledge_flushed(&empty_checkpoint);
    queue.journal_entries(&mut entries);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.covered.contains(&(0, 0)),
        "new pending ticks preserve coverage"
    );
    queue.drain_ready(20, 1, |_| true);
    queue.evict_flushed(|_, _| true);
    assert!(
        queue.covered.contains(&(0, 0)),
        "dirty consumed queue needs a new acknowledgement"
    );
}

#[test]
fn durable_images_start_clean_and_project_changes_once() {
    let dir = directory();
    let cpos = ChunkPos { x: 0, z: 0 };
    let want = [(cpos, 0)].into_iter().collect();
    let mut world = world(Some(dir.clone()));
    world.ensure_blocking(&want);
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);

    // Durable empty coverage needs no native projection.
    let empty = BlockTicksImage {
        next_sequence: 0,
        chunks: vec![BlockTickChunkImage {
            chunk: [0, 0],
            ticks: Vec::new(),
        }],
    };
    let queue = BlockTicks::from_image(empty.clone(), Some(&empty));
    assert!(queue.checkpoint_dirty.is_empty());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );

    // Durable nonempty coverage needs no projection either.
    let mut seeded = BlockTicks::default();
    seeded.schedule(tick(1, 10, 0));
    let base = seeded.image();
    let queue = BlockTicks::from_image(base.clone(), Some(&base));
    assert!(queue.checkpoint_dirty.is_empty());
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );

    // A reordered durable chunk image is still semantically equal.
    seeded.schedule(tick(2, 11, 0));

    let two = seeded.image();
    let mut reordered = two.clone();
    reordered.chunks[0].ticks.reverse();
    let queue = BlockTicks::from_image(reordered, Some(&two));
    assert!(queue.checkpoint_dirty.is_empty());
    assert!(queue.checkpoint(&mut world, 5).is_empty());

    // Changed queues project once; journal drain is not an ACK, and a
    // matching ACK makes the repeated projection empty.
    let mut queue = BlockTicks::from_image(base.clone(), Some(&base));
    queue.schedule(tick(2, 20, 0));
    let changed = queue.image();
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    // Resident chunks project in place: `checkpoint` returns only
    // non-resident images, so dirtiness is asserted on the capture.
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    assert!(
        !world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    assert!(!entries.is_empty());
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    // Repeating the projection with identical content is a world-side no-op
    // (field equality fence): retry does not rewrite the chunk every save.
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);
    queue.acknowledge_flushed(&changed);
    assert!(queue.checkpoint_dirty.is_empty());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    queue.acknowledge_flushed(&changed);
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    drop(world);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn unknown_base_starts_pending_and_failed_capture_retries() {
    let dir = directory();
    let cpos = ChunkPos { x: 0, z: 0 };
    let want = [(cpos, 0)].into_iter().collect();
    let mut world = world(Some(dir.clone()));
    world.ensure_blocking(&want);
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);

    let mut seeded = BlockTicks::default();
    seeded.schedule(tick(1, 10, 0));
    let image = seeded.image();
    let mut queue = BlockTicks::from_image(image.clone(), None);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    // Failed capture (no ACK) keeps the projection pending for retry.
    queue.checkpoint(&mut world, 5);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    assert!(
        !world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);
    queue.checkpoint(&mut world, 5);
    let written = world.capture_dirty(&Default::default()).written();
    world.mark_flushed(&written);
    queue.acknowledge_flushed(&image);
    assert!(queue.checkpoint_dirty.is_empty());
    assert!(queue.checkpoint(&mut world, 5).is_empty());
    assert!(
        world
            .capture_dirty(&Default::default())
            .written()
            .is_empty()
    );
    drop(world);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn stale_ack_after_newer_state_keeps_projection_pending() {
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 10, 0));
    let base = queue.image();
    let mut queue = BlockTicks::from_image(base.clone(), Some(&base));
    assert!(queue.checkpoint_dirty.is_empty());
    queue.schedule(tick(2, 20, 0));
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    // A stale ACK for the older equal image does not clear newer state.
    queue.acknowledge_flushed(&base);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    let changed = queue.image();
    queue.drain_ready(20, 10, |_| true);
    queue.acknowledge_flushed(&changed);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    let drained = queue.image();
    queue.acknowledge_flushed(&drained);
    assert!(queue.checkpoint_dirty.is_empty());
    // Native ACK alone cannot discard queue changes still waiting for WAL.
    queue.acknowledge_flushed(&drained);
    queue.evict_flushed(|_, _| true);
    assert!(queue.covered.contains(&(0, 0)));
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    assert!(!entries.is_empty());
    queue.evict_flushed(|_, _| true);
    assert!(!queue.covered.contains(&(0, 0)));
}

#[test]
fn wal_only_images_project_once_and_sequence_alone_is_clean() {
    let mut queue = BlockTicks::default();
    queue.schedule(tick(1, 10, 0));
    let mut entries = Vec::new();
    queue.journal_entries(&mut entries);
    assert!(!entries.is_empty());
    let image = queue.image();
    // WAL-only images prove no durable projection.
    let mut queue = BlockTicks::from_image(image, None);
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    queue.schedule(tick(2, 20, 0));
    queue.drain_ready(20, 10, |_| true);
    let changed = queue.image();
    assert!(queue.checkpoint_dirty.contains(&(0, 0)));
    queue.acknowledge_flushed(&changed);
    assert!(queue.checkpoint_dirty.is_empty());
    // A manifest bump of only `next_sequence` is not a content change.
    let mut bumped = changed.clone();
    bumped.next_sequence += 1;
    let clean = BlockTicks::from_image(bumped.clone(), Some(&changed));
    assert!(clean.checkpoint_dirty.is_empty());
}
