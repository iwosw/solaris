use super::super::definition::{DefinitionInput, PlaneBounds};
use super::super::prepared::{PrepareOptions, PreparedReadOnlyWorld, PreparedWorld, prepare};
use super::super::region_cache::RegionCache;
use super::*;

fn blocks() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}
fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}
fn bytes(chunk: &Chunk, blocks: &BlockRegistry) -> Vec<u8> {
    mc_world::anvil::chunk_to_payload_with_items_at_tick_for_position(
        chunk, chunk.pos, blocks, None, 0, 0,
    )
    .unwrap()
    .uncompressed_nbt
}
fn plane_definition() -> WorldDefinition {
    let mut input = DefinitionInput::earth_root(42);
    input.domain = PlaneBounds {
        min_x: -32,
        min_z: -32,
        max_x_exclusive: 32,
        max_z_exclusive: 32,
    };
    input.macro_cell_m = 16.0;
    input.climate_cell_m = 32.0;
    input.validate().unwrap()
}

#[test]
fn basin_base_matches_existing_materialization_without_gameplay_initialization() {
    let directory = tempfile::tempdir().unwrap();
    let definition = DefinitionInput::basin_fixture(42).validate().unwrap();
    let world = prepare(
        directory.path(),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
    )
    .unwrap();
    let blocks = blocks();
    let materializer = BaseMaterializer::new(
        world.definition(),
        world.identity(),
        world.root_plan(),
        &blocks,
    )
    .unwrap();
    for pos in [ChunkPos { x: 2, z: 2 }, ChunkPos { x: 2, z: 4 }] {
        let ready = materializer.prepare_chunk(pos, &blocks).unwrap();
        let chunk = ready.materialize_chunk().unwrap();
        let original = world
            .root_plan()
            .basin_fixture()
            .unwrap()
            .materialize_chunk(pos, &blocks)
            .unwrap();
        assert_eq!(bytes(&chunk, &blocks), bytes(&original, &blocks));
        assert!(!chunk.dirty);
        assert_eq!(chunk.world_journal_lsn(), 0);
        assert!(chunk.generated_entities().is_empty());
        assert!(chunk.chests.is_empty());
        for section_y in MIN_Y / 16..MAX_Y / 16 {
            let section = ready.materialize_section(section_y).unwrap();
            for (x, y, z) in [(0, 0, 0), (7, 8, 8), (15, 15, 15)] {
                assert_eq!(
                    section.get(x, y, z),
                    original
                        .get_block(x, section_y * 16 + i32::from(y), z)
                        .unwrap()
                );
            }
        }
        assert_eq!(ready.sample(16, 0, 0), None);
        assert_eq!(ready.sample(0, MAX_Y, 0), None);
        assert!(ready.materialize_section(MIN_Y / 16 - 1).is_err());
    }
    assert_eq!(world.store().record_counts().unwrap(), Default::default());
    assert!(
        materializer
            .prepare_chunk(ChunkPos { x: -1, z: 0 }, &blocks)
            .is_err()
    );
}

#[test]
fn ready_plane_uses_actual_region_water_and_geology_at_negative_coordinates() {
    let definition = plane_definition();
    let root = WorldPlan::build(&definition).unwrap();
    let identity = WorldIdentity::derive(
        &definition,
        super::super::definition::algorithm_artifact_id(),
        super::super::definition::hash(&root.canonical_bytes().unwrap()),
    )
    .unwrap();
    let blocks = blocks();
    let materializer = BaseMaterializer::new(&definition, identity, &root, &blocks).unwrap();
    let position = ChunkPos { x: -1, z: -1 };
    let mut prepared_count = 0;
    let ready = materializer
        .prepare_chunk_with(position, &blocks, |planner, node| {
            prepared_count += 1;
            planner.prepare(node).map(Arc::new)
        })
        .unwrap();
    assert!(prepared_count <= MAX_REGION_PREPARES_PER_CHUNK);
    let planner = RegionPlanner::new(root.plane().unwrap(), identity);
    for (x, z) in [(0u8, 0u8), (7, 8), (15, 15)] {
        let x_m = f64::from(-16 + i32::from(x));
        let z_m = f64::from(-16 + i32::from(z));
        let node = planner.leaf_at(x_m, z_m).unwrap();
        let column = planner.prepare(node).unwrap().sample(x_m, z_m).unwrap();
        let basement = match column.geology_ecology.geology.bands[0].lithology {
            RockClass::Basalt => "minecraft:basalt",
            RockClass::Granite => "minecraft:granite",
            RockClass::Intrusive => "minecraft:diorite",
            RockClass::Metamorphic => "minecraft:deepslate",
            other => panic!("unexpected physical basement: {other:?}"),
        };
        assert_eq!(ready.sample(x, MIN_Y, z), Some(state(&blocks, basement)));
        assert_ne!(
            ready.sample(x, column.ground_y, z),
            Some(state(&blocks, "minecraft:air"))
        );
        assert_eq!(
            ready.sample(x, column.water_surface_y.unwrap_or(column.ground_y) + 1, z),
            Some(state(&blocks, "minecraft:air"))
        );
        if let Some(surface) = column.water_surface_y {
            assert!(surface > column.ground_y);
            assert_eq!(
                ready.sample(x, surface, z),
                Some(state(&blocks, "minecraft:water"))
            );
        }
    }
    // Sampling and materializing remain usable after all planners and root data go away.
    drop(root);
    let chunk = ready.materialize_chunk().unwrap();
    assert!(!chunk.dirty);
    assert_eq!(chunk.world_journal_lsn(), 0);
    assert!(chunk.scheduled_block_ticks().is_empty());
    assert!(chunk.scheduled_fluid_ticks().is_empty());
}

#[test]
fn p08_real_region_cache_purge_regenerates_base_and_retains_offline_edit_read_only() {
    let directory = tempfile::tempdir().unwrap();
    let definition = plane_definition();
    let blocks = blocks();
    let items = mc_data::items::solaris_required_items();
    let world = prepare(
        directory.path(),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
    )
    .unwrap();
    let identity = world.identity();
    let cache_path = directory.path().join("derived-regions.redb");
    let cache = RegionCache::create(&cache_path, 4 * 1024 * 1024, 1024 * 1024).unwrap();
    let materializer =
        BaseMaterializer::new(world.definition(), identity, world.root_plan(), &blocks).unwrap();
    let position = ChunkPos { x: -1, z: -1 };
    let ready = materializer
        .prepare_chunk_with(position, &blocks, |planner, node| {
            if let Some(plan) = cache.get(identity, node).unwrap() {
                return Ok(Arc::new(plan));
            }
            let plan = planner.prepare(node)?;
            cache.insert(&plan).unwrap();
            Ok(Arc::new(plan))
        })
        .unwrap();
    let base = ready.materialize_chunk().unwrap();
    let base_bytes = bytes(&base, &blocks);
    assert_eq!(world.store().record_counts().unwrap(), Default::default());
    let mut effective = world.store().compose(0, &base, &blocks, &items).unwrap();
    let air = state(&blocks, "minecraft:air");
    effective.set_block(3, MIN_Y + 4, 5, air).unwrap();
    let first_lsn = world
        .store()
        .apply_offline_snapshot(0, &base, &mut effective, &blocks, &items)
        .unwrap();
    assert!(first_lsn > 0);
    let changed = state(&blocks, "minecraft:gold_block");
    effective.set_block(3, MIN_Y + 4, 5, changed).unwrap();
    let second_lsn = world
        .store()
        .apply_offline_snapshot(0, &base, &mut effective, &blocks, &items)
        .unwrap();
    assert!(second_lsn > first_lsn);
    drop(world);
    drop(cache);
    std::fs::remove_file(&cache_path).unwrap();
    let state_path = directory.path().join("world-artifacts/state.redb");
    let native_before = std::fs::read(&state_path).unwrap();
    let read_only =
        PreparedReadOnlyWorld::open(directory.path(), None, PrepareOptions::default()).unwrap();
    assert!(PreparedWorld::open(directory.path(), None, PrepareOptions::default()).is_err());
    let report = read_only.inspection();
    let regenerated = BaseMaterializer::new(
        &report.definition,
        report.identity,
        &report.root_plan,
        &blocks,
    )
    .unwrap()
    .prepare_chunk_with(position, &blocks, |planner, node| {
        planner.prepare(node).map(Arc::new)
    })
    .unwrap()
    .materialize_chunk()
    .unwrap();
    assert_eq!(bytes(&regenerated, &blocks), base_bytes);
    let resolved = read_only
        .store()
        .resolve_base(0, position, &blocks, &items, || Ok(Some(regenerated)))
        .unwrap()
        .unwrap();
    let composed = read_only
        .store()
        .compose(0, &resolved, &blocks, &items)
        .unwrap();
    assert_eq!(composed.get_block(3, MIN_Y + 4, 5), Some(changed));
    assert_eq!(composed.world_journal_lsn(), second_lsn);
    assert_eq!(
        composed.get_block(4, MIN_Y + 4, 5),
        base.get_block(4, MIN_Y + 4, 5)
    );
    assert_eq!(read_only.store().record_counts().unwrap(), report.counts);
    drop(read_only);
    assert_eq!(std::fs::read(&state_path).unwrap(), native_before);
    assert!(
        !cache_path.exists(),
        "inspection must not create a discardable cache"
    );
}

#[test]
fn committed_read_only_materialization_refuses_missing_native_state() {
    let directory = tempfile::tempdir().unwrap();
    let definition = DefinitionInput::basin_fixture(42).validate().unwrap();
    let world = prepare(
        directory.path(),
        &definition,
        definition.input().domain,
        PrepareOptions::default(),
    )
    .unwrap();
    drop(world);
    std::fs::remove_file(directory.path().join("world-artifacts/state.redb")).unwrap();
    assert!(
        PreparedReadOnlyWorld::open(directory.path(), None, PrepareOptions::default()).is_err()
    );
    assert!(!directory.path().join("world-artifacts/state.redb").exists());
}

#[test]
fn native_schema_one_manifest_fails_before_algorithm_identity_validation() {
    for explicit_schema in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let definition = DefinitionInput::basin_fixture(42).validate().unwrap();
        drop(
            prepare(
                directory.path(),
                &definition,
                definition.input().domain,
                PrepareOptions::default(),
            )
            .unwrap(),
        );
        let manifest_path = directory
            .path()
            .join(super::super::prepared::WORLD_MANIFEST);
        let mut manifest: toml::Table =
            toml::from_str(&std::fs::read_to_string(&manifest_path).unwrap()).unwrap();
        if explicit_schema {
            manifest.insert("native_schema".into(), toml::Value::Integer(1));
        } else {
            manifest.remove("native_schema");
        }
        manifest.insert(
            "algorithm_artifact".into(),
            toml::Value::Array(vec![toml::Value::Integer(0); 32]),
        );
        std::fs::write(&manifest_path, toml::to_string(&manifest).unwrap()).unwrap();
        let error =
            match PreparedReadOnlyWorld::open(directory.path(), None, PrepareOptions::default()) {
                Ok(_) => panic!("schema-one world must not be reopened as current native state"),
                Err(error) => error,
            };
        assert!(matches!(
            error,
            PrepareError::Native(mc_world::native::NativeError::UnsupportedSchema(1))
        ));
    }
}

#[test]
fn reviewer_p2_partial_physical_boundary_chunks_keep_valid_cells_and_deny_padding() {
    let directory = tempfile::tempdir().unwrap();
    let mut input = DefinitionInput::earth_root(42);
    input.domain = PlaneBounds {
        min_x: -33,
        min_z: -33,
        max_x_exclusive: 33,
        max_z_exclusive: 33,
    };
    input.macro_cell_m = 16.0;
    input.climate_cell_m = 32.0;
    let definition = input.validate().unwrap();
    let playable = PlaneBounds {
        min_x: -32,
        min_z: -32,
        max_x_exclusive: 32,
        max_z_exclusive: 32,
    };
    let world = prepare(
        directory.path(),
        &definition,
        playable,
        PrepareOptions::default(),
    )
    .unwrap();
    let blocks = blocks();
    let items = mc_data::items::solaris_required_items();
    let materializer = BaseMaterializer::new(
        world.definition(),
        world.identity(),
        world.root_plan(),
        &blocks,
    )
    .unwrap();
    for (chunk_x, valid_x, padding_x) in [(2, 0, 1), (-3, 15, 14)] {
        let ready = materializer
            .prepare_chunk(ChunkPos { x: chunk_x, z: 0 }, &blocks)
            .unwrap();
        assert!(ready.sample(valid_x, -60, 0).is_some());
        assert_eq!(ready.sample(padding_x, -60, 0), None);
        let base = ready.into_chunk().unwrap();
        let mut effective = base.clone();
        effective
            .set_block(valid_x, -60, 0, state(&blocks, "minecraft:air"))
            .unwrap();
        world
            .store()
            .apply_offline_snapshot(0, &base, &mut effective, &blocks, &items)
            .unwrap();
        let actual = world.store().compose(0, &base, &blocks, &items).unwrap();
        assert_eq!(
            actual.get_block(valid_x, -60, 0),
            Some(state(&blocks, "minecraft:air"))
        );
        assert_eq!(
            actual.get_block(padding_x, -60, 0),
            Some(state(&blocks, "minecraft:air"))
        );
    }
}

#[test]
fn overlapping_tree_canopies_preserve_trunks_and_nearest_leaf_distance() {
    let blocks = blocks();
    let palette = Palette::new(&blocks).unwrap();
    let position = ChunkPos { x: 0, z: 0 };
    let mut ready = PreparedBaseChunk {
        position,
        identity: WorldIdentity::derive(&plane_definition(), [0; 32], [0; 32]).unwrap(),
        palette,
        base: ReadyBase::Basin(Box::new(Chunk::empty(
            position,
            palette.air,
            Identifier::parse("minecraft:plains").unwrap(),
        ))),
        trees: vec![
            Tree {
                x: 6,
                z: 6,
                ground_y: 0,
                height: 4,
                kind: 0,
            },
            Tree {
                x: 7,
                z: 6,
                ground_y: 0,
                height: 5,
                kind: 1,
            },
        ],
    };
    assert_eq!(ready.sample(7, 4, 6), Some(palette.log[1]));
    assert_eq!(ready.sample(7, 5, 7), Some(palette.leaves[1][0]));
    assert_eq!(ready.sample(7, 6, 7), Some(palette.leaves[1][1]));
    ready.trees.reverse();
    assert_eq!(ready.sample(7, 4, 6), Some(palette.log[1]));
    assert_eq!(ready.sample(7, 5, 7), Some(palette.leaves[1][0]));
    assert_eq!(ready.sample(7, 6, 7), Some(palette.leaves[1][1]));
}

#[test]
fn soluble_karst_preserves_protected_and_water_bearing_columns() {
    let palette = Palette::new(&blocks()).unwrap();
    let carbonate = palette.rocks[4];
    let mut column = RenderedColumn {
        ground_y: 32,
        water_y: None,
        protected: false,
        bands: [RenderedBand {
            bottom_y: MIN_Y,
            top_y: 32,
            state: carbonate,
            present: true,
            solubility: 1.0,
        }; 5],
        surface: carbonate,
        soil: false,
        karst_conduit: 0.0,
        mineralization: 0.0,
        intrusion: 0.0,
        world_x: 0,
        world_z: 0,
        seed: 42,
    };
    assert_eq!(column.sample(-9, &palette), palette.air);
    column.protected = true;
    assert_ne!(column.sample(-9, &palette), palette.air);
    column.protected = false;
    column.water_y = Some(33);
    assert_ne!(column.sample(-9, &palette), palette.air);
    assert_eq!(column.sample(33, &palette), palette.water);
    assert_eq!(column.sample(34, &palette), palette.air);
    assert_eq!(column.sample(MIN_Y, &palette), carbonate);
}
