use std::io::{BufRead, Read, Write};
use std::process::{Command, Stdio};

use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_world::native::NativeChunkSnapshot;
use mc_world::{BlockRegistry, Chunk, ChunkPos};

use super::super::basin_fixture::BasinFixture;
use super::*;

fn definition() -> WorldDefinition {
    DefinitionInput::basin_fixture(42).validate().unwrap()
}
fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}
fn items() -> ItemRegistry {
    mc_data::items::solaris_required_items()
}
fn base_bytes(chunk: &Chunk, blocks: &BlockRegistry) -> Vec<u8> {
    mc_world::anvil::chunk_to_payload_with_items_at_tick_for_position(
        chunk, chunk.pos, blocks, None, 0, 0,
    )
    .unwrap()
    .uncompressed_nbt
}
fn options(cache: usize) -> PrepareOptions {
    PrepareOptions {
        native_cache_bytes: cache,
        ..PrepareOptions::default()
    }
}

#[test]
fn p01_cache_budgets_preserve_identity_root_facts_and_all_base_bytes() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let definition = definition();
    let a = prepare(
        first.path(),
        &definition,
        definition.input().domain,
        options(2 * 1024 * 1024),
    )
    .unwrap();
    let b = prepare(
        second.path(),
        &definition,
        definition.input().domain,
        options(16 * 1024 * 1024),
    )
    .unwrap();
    assert_eq!(a.identity(), b.identity());
    assert_eq!(a.root_plan(), b.root_plan());
    assert_eq!(
        std::fs::read(first.path().join(ARTIFACTS).join(ROOT)).unwrap(),
        std::fs::read(second.path().join(ARTIFACTS).join(ROOT)).unwrap()
    );
    let blocks = registry();
    for z in 0..5 {
        for x in 0..5 {
            let pos = ChunkPos { x, z };
            let a_base = a
                .store()
                .resolve_base(
                    0,
                    pos,
                    &blocks,
                    &items(),
                    &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                    || {
                        Ok(a.root_plan()
                            .basin_fixture()
                            .unwrap()
                            .materialize_chunk(pos, &blocks))
                    },
                )
                .unwrap()
                .unwrap();
            let b_base = b
                .store()
                .resolve_base(
                    0,
                    pos,
                    &blocks,
                    &items(),
                    &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
                    || {
                        Ok(b.root_plan()
                            .basin_fixture()
                            .unwrap()
                            .materialize_chunk(pos, &blocks))
                    },
                )
                .unwrap()
                .unwrap();
            assert_eq!(base_bytes(&a_base, &blocks), base_bytes(&b_base, &blocks));
        }
    }
    assert_eq!(
        a.store().record_counts().unwrap(),
        mc_world::native::NativeRecordCounts::default()
    );
    assert_eq!(
        b.store().record_counts().unwrap(),
        mc_world::native::NativeRecordCounts::default()
    );
}

#[test]
fn p02_bounds_expand_shrink_retains_base_coordinates_and_durable_saves() {
    let root = tempfile::tempdir().unwrap();
    let definition = definition();
    let blocks = Arc::new(registry());
    let pos = ChunkPos { x: 2, z: 2 };
    let initial = PlaneBounds {
        min_x: 32,
        max_x_exclusive: 48,
        min_z: 32,
        max_z_exclusive: 48,
    };
    let mut world = prepare(root.path(), &definition, initial, PrepareOptions::default()).unwrap();
    let identity = world.identity();
    let base = world
        .root_plan()
        .basin_fixture()
        .unwrap()
        .materialize_chunk(pos, &blocks)
        .unwrap();
    let before = base_bytes(&base, &blocks);
    let air = blocks
        .block(&Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let mut edited = base.clone();
    edited.set_block(1, 92, 1, air).unwrap();
    edited
        .extras
        .push(("SolarisJournalLsn".into(), mc_nbt::Tag::Long(7)));
    world
        .store()
        .apply_snapshots(
            &[NativeChunkSnapshot {
                dimension: 0,
                base: &base,
                effective: &edited,
            }],
            &blocks,
            &items(),
        )
        .unwrap();
    let counts = world.store().record_counts().unwrap();
    assert_eq!(counts.overlay_sections, 1);
    let expanded = world.set_bounds(definition.input().domain).unwrap();
    assert_eq!(expanded.operating_revision, 2);
    let shrunk = world
        .set_bounds(PlaneBounds {
            min_x: 0,
            max_x_exclusive: 16,
            min_z: 0,
            max_z_exclusive: 16,
        })
        .unwrap();
    assert_eq!(shrunk.operating_revision, 3);
    assert_eq!(world.identity(), identity);
    drop(world);
    let world =
        PreparedWorld::open(root.path(), Some(&definition), options(2 * 1024 * 1024)).unwrap();
    assert_eq!(world.playable(), shrunk);
    assert_eq!(world.identity(), identity);
    assert_eq!(world.store().record_counts().unwrap(), counts);
    let reopened_base = world
        .root_plan()
        .basin_fixture()
        .unwrap()
        .materialize_chunk(pos, &blocks)
        .unwrap();
    assert_eq!(base_bytes(&reopened_base, &blocks), before);
    let effective = world
        .store()
        .compose(
            0,
            &reopened_base,
            &blocks,
            &items(),
            &mc_data::loot::chest_26_1_2::ChestLootCatalog::new(),
        )
        .unwrap();
    assert_eq!(effective.get_block(1, 92, 1), Some(air));
    assert_eq!(effective.world_journal_lsn(), 7);
    let invalid = PlaneBounds {
        min_x: -16,
        ..initial
    };
    assert!(world.definition().input().domain.contains(initial));
    drop(world);
    let mut world = PreparedWorld::open(root.path(), None, PrepareOptions::default()).unwrap();
    assert!(world.set_bounds(invalid).is_err());
    assert_eq!(world.playable(), shrunk);
}

#[test]
fn p03_unvisited_upstream_outside_playable_contributes_full_discharge() {
    let root = tempfile::tempdir().unwrap();
    let definition = definition();
    let bounds = PlaneBounds {
        min_x: 32,
        max_x_exclusive: 48,
        min_z: 64,
        max_z_exclusive: 80,
    };
    let world = prepare(root.path(), &definition, bounds, PrepareOptions::default()).unwrap();
    assert_eq!(
        world.root_plan().basin_fixture().unwrap().cells()[22].discharge,
        25
    );
    assert_eq!(
        world.root_plan().basin_fixture().unwrap().cells()[22].downstream,
        None
    );
    let blocks = registry();
    let outlet = world
        .root_plan()
        .basin_fixture()
        .unwrap()
        .materialize_chunk(ChunkPos { x: 2, z: 4 }, &blocks)
        .unwrap();
    let water = blocks
        .block(&Identifier::parse("minecraft:water").unwrap())
        .unwrap()
        .default;
    assert_eq!(outlet.get_block(7, 88, 7), Some(water));
    assert_eq!(
        world.store().record_counts().unwrap(),
        mc_world::native::NativeRecordCounts::default()
    );
    assert_eq!(world.playable().bounds, bounds);
}

#[test]
fn p04_extent_resolution_seed_and_scale_changes_refuse_startup() {
    let root = tempfile::tempdir().unwrap();
    let definition = definition();
    drop(
        prepare(
            root.path(),
            &definition,
            definition.input().domain,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let original_root = std::fs::read(root.path().join(ARTIFACTS).join(ROOT)).unwrap();
    for change in 0..4 {
        let mut input = definition.input().clone();
        match change {
            0 => input.domain.max_x_exclusive = 96,
            1 => input.macro_cell_m = 32.0,
            2 => input.seed += 1,
            _ => input.horizontal_meters_per_block = 2.0,
        }
        let changed = input.validate().unwrap();
        assert!(matches!(
            PreparedWorld::open(root.path(), Some(&changed), PrepareOptions::default()),
            Err(PrepareError::IdentityMismatch)
        ));
    }
    assert_eq!(
        std::fs::read(root.path().join(ARTIFACTS).join(ROOT)).unwrap(),
        original_root
    );
}

#[test]
fn roundtrip_preserves_canonical_lake_root_and_store_identity() {
    let root = tempfile::tempdir().unwrap();
    let definition = definition();
    let world = prepare(
        root.path(),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
    )
    .unwrap();
    let identity = world.identity();
    drop(world);
    let world =
        PreparedWorld::open(root.path(), Some(&definition), PrepareOptions::default()).unwrap();
    assert_eq!(world.identity(), identity);
    assert_eq!(
        world.root_plan().basin_fixture().unwrap().cells()[12].ground_y,
        82
    );
    assert_eq!(
        world.root_plan().basin_fixture().unwrap().cells()[12].spill_y,
        92
    );
    assert_eq!(
        world.root_plan().basin_fixture().unwrap().cells()[12].downstream,
        Some(17)
    );
    assert_eq!(
        world.root_plan().canonical_bytes().unwrap(),
        std::fs::read(root.path().join(ARTIFACTS).join(ROOT)).unwrap()
    );
    assert_eq!(world.definition(), &definition);
    assert_eq!(publication_state(root.path()), PublicationState::Committed);
}

#[test]
fn committed_missing_or_corrupt_root_store_and_bounds_fail_closed() {
    for file in [ROOT, NATIVE, BOUNDS] {
        for corrupt in [false, true] {
            let root = tempfile::tempdir().unwrap();
            let definition = definition();
            drop(
                prepare(
                    root.path(),
                    &definition,
                    definition.input().domain,
                    PrepareOptions::default(),
                )
                .unwrap(),
            );
            let path = root.path().join(ARTIFACTS).join(file);
            if corrupt {
                std::fs::write(&path, b"corrupt").unwrap();
            } else {
                std::fs::remove_file(&path).unwrap();
            }
            let result =
                PreparedWorld::open(root.path(), Some(&definition), PrepareOptions::default());
            assert!(
                matches!(result, Err(PrepareError::Committed { .. })),
                "file={file}, corrupt={corrupt}"
            );
            assert!(
                prepare(
                    root.path(),
                    &definition,
                    definition.input().domain,
                    PrepareOptions::default()
                )
                .is_err()
            );
            assert_eq!(publication_state(root.path()), PublicationState::Committed);
            if !corrupt {
                assert!(!path.exists());
            }
        }
    }
}

#[test]
fn prepare_crash_helper() {
    let Some(root) = std::env::var_os("SOLARIS_W1_CRASH_ROOT") else {
        return;
    };
    let phase: usize = std::env::var("SOLARIS_W1_CRASH_PHASE")
        .unwrap()
        .parse()
        .unwrap();
    let definition = definition();
    let _world = prepare_observed(
        Path::new(&root),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
        |found| {
            if found as usize == phase {
                println!("W1_CRASH_READY");
                std::io::stdout().flush().unwrap();
                std::io::stdin().read_exact(&mut [0_u8]).unwrap();
            }
            Ok(())
        },
    )
    .unwrap();
}

#[test]
fn prepare_process_crash_recovers_each_durable_publication_boundary() {
    for (index, expected) in [
        PublicationState::OrphanStaging,
        PublicationState::OrphanStaging,
        PublicationState::OrphanStaging,
        PublicationState::OrphanPrepared,
        PublicationState::Committed,
    ]
    .into_iter()
    .enumerate()
    {
        let root = tempfile::tempdir().unwrap();
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "earth::prepared::tests::prepare_crash_helper",
                "--nocapture",
                "--test-threads=1",
            ])
            .env("SOLARIS_W1_CRASH_ROOT", root.path())
            .env("SOLARIS_W1_CRASH_PHASE", index.to_string())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let mut reader = std::io::BufReader::new(child.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                reader.read_line(&mut line).unwrap(),
                0,
                "crash helper exited before publication boundary"
            );
            if line.contains("W1_CRASH_READY") {
                break;
            }
        }
        let definition = definition();
        assert!(matches!(
            PreparedWorld::open(root.path(), None, PrepareOptions::default()),
            Err(PrepareError::Lease(WorldError::WorldLocked { .. }))
        ));
        child.kill().unwrap();
        child.wait().unwrap();
        assert_eq!(publication_state(root.path()), expected);
        if expected != PublicationState::Committed {
            assert!(PreparedWorld::open(root.path(), None, PrepareOptions::default()).is_err());
            let changed = DefinitionInput::basin_fixture(43).validate().unwrap();
            assert!(matches!(
                prepare(
                    root.path(),
                    &changed,
                    changed.input().domain,
                    PrepareOptions::default()
                ),
                Err(PrepareError::IdentityMismatch)
            ));
        }
        let world = prepare(
            root.path(),
            &definition,
            definition.input().domain,
            PrepareOptions::default(),
        )
        .unwrap();
        assert_eq!(
            world.root_plan().basin_fixture().unwrap().cells()[22].discharge,
            25
        );
        assert_eq!(publication_state(root.path()), PublicationState::Committed);
    }
}

#[test]
fn checked_inputs_reject_unsupported_and_preallocation_overflow() {
    for change in 0..9 {
        let mut input = DefinitionInput::basin_fixture(42);
        match change {
            0 => input.horizontal_meters_per_block = 0.0,
            1 => input.macro_cell_m = f64::NAN,
            2 => input.climate_cell_m = -1.0,
            3 => input.vertical_profile = "metric".into(),
            4 => input.topology = "sphere".into(),
            5 => input.domain.min_x = i64::MIN,
            6 => input.domain.max_z_exclusive = 0,
            7 => input.horizontal_meters_per_block = f64::MAX,
            _ => input.macro_cell_m = f64::MIN_POSITIVE,
        }
        assert!(input.validate().is_err());
    }
    let definition = definition();
    let root = tempfile::tempdir().unwrap();
    let world = root.path().join("world");
    assert!(
        prepare(
            &world,
            &definition,
            definition.input().domain,
            PrepareOptions {
                build_scratch_limit_bytes: 1,
                ..PrepareOptions::default()
            }
        )
        .is_err()
    );
    assert!(!world.exists());
}

#[test]
fn small_and_earth_reference_estimates_report_area_and_partial_edges_without_allocation() {
    let mut input = DefinitionInput::basin_fixture(42);
    input.domain = PlaneBounds {
        min_x: -8192,
        max_x_exclusive: 8192,
        min_z: -8192,
        max_z_exclusive: 8192,
    };
    input.macro_cell_m = 256.0;
    input.climate_cell_m = 1024.0;
    let estimate = input.clone().validate().unwrap().estimate().unwrap();
    assert_eq!(estimate.root_cells, 4096);
    assert_eq!(estimate.climate_cells, 256);
    assert_eq!(estimate.area_m2, 268435456.0);
    input.domain = PlaneBounds {
        min_x: -15970288,
        max_x_exclusive: 15970288,
        min_z: -7985144,
        max_z_exclusive: 7985144,
    };
    input.macro_cell_m = 16384.0;
    input.climate_cell_m = 65536.0;
    let estimate = input.validate().unwrap().estimate().unwrap();
    assert_eq!((estimate.root_columns, estimate.root_rows), (1950, 975));
    assert_eq!(estimate.climate_cells, 488 * 244);
    assert_eq!(estimate.area_m2, 510100197605888.0);
    assert_eq!(estimate.root_partial_edge_x_m, 8160.0);
    assert_eq!(estimate.root_partial_edge_z_m, 12272.0);
}

#[test]
fn root_decode_rejects_corruption_and_nonconservative_foreign_keys() {
    let bytes = BasinFixture::build().canonical_bytes();
    for (offset, value) in [(0, 0_u32), (20 + 12, 99), (20 + 8, 99), (20 + 16, 99)] {
        let mut corrupt = bytes.clone();
        corrupt[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
        assert!(BasinFixture::from_canonical_bytes(&corrupt).is_err());
    }
    assert!(BasinFixture::from_canonical_bytes(&bytes[..519]).is_err());
}

#[test]
fn committed_inspection_is_read_only_and_lease_exclusive() {
    let root = tempfile::tempdir().unwrap();
    let definition = definition();
    let world = prepare(
        root.path(),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
    )
    .unwrap();
    assert!(matches!(
        inspect_committed(root.path(), None, PrepareOptions::default()),
        Err(PrepareError::Lease(WorldError::WorldLocked { .. }))
    ));
    let identity = world.identity();
    drop(world);
    let paths = [
        root.path().join(WORLD_MANIFEST),
        root.path().join(ARTIFACTS).join(ROOT),
        root.path().join(ARTIFACTS).join(NATIVE),
        root.path().join(ARTIFACTS).join(BOUNDS),
    ];
    let before: Vec<_> = paths
        .iter()
        .map(|path| std::fs::read(path).unwrap())
        .collect();
    let report =
        inspect_committed(root.path(), Some(&definition), PrepareOptions::default()).unwrap();
    assert_eq!(report.identity, identity);
    assert_eq!(
        report.root_plan.basin_fixture().unwrap().cells()[12].spill_y,
        92
    );
    for (path, bytes) in paths.iter().zip(before) {
        assert_eq!(
            std::fs::read(path).unwrap(),
            bytes,
            "inspection changed {}",
            path.display()
        );
    }
}
