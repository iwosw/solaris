use super::*;
use crate::earth::definition::{DefinitionInput, PlaneBounds};
use crate::earth::prepared::{PrepareOptions, prepare};

fn definition() -> WorldDefinition {
    let mut input = DefinitionInput::earth_root(42);
    input.domain = PlaneBounds {
        min_x: -64,
        min_z: -64,
        max_x_exclusive: 64,
        max_z_exclusive: 64,
    };
    input.macro_cell_m = 16.0;
    input.climate_cell_m = 32.0;
    input.validate().unwrap()
}

fn registry() -> Arc<BlockRegistry> {
    Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    )
}

fn state(blocks: &BlockRegistry, name: &str) -> BlockStateId {
    blocks
        .block(&Identifier::parse(name).unwrap())
        .unwrap()
        .default
}

#[test]
fn native_and_borrowed_source_agree_on_natural_water_without_activation() {
    let directory = tempfile::tempdir().unwrap();
    let definition = definition();
    let world = Arc::new(
        prepare(
            directory.path(),
            &definition,
            definition.input().domain,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let blocks = registry();
    let items = Arc::new(mc_data::items::solaris_required_items());
    let loot = ChestLootCatalog::new();
    let cache = EarthSourceCache::default();
    let borrowed = EarthBaseSource::new(
        world.definition(),
        world.identity(),
        world.root_plan(),
        &blocks,
        None,
        StructureLoot {
            seed: 42,
            catalog: &loot,
            items: &items,
        },
        &cache,
    )
    .unwrap();
    let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None)
        .unwrap()
        .with_chest_loot(loot.clone(), items.clone());
    let position = ChunkPos { x: -1, z: -1 };
    let pure = borrowed.materialize_chunk(position).unwrap();
    let base = runtime.load_base(0, position).unwrap().unwrap();
    let water = state(&blocks, "minecraft:water");
    for z in 0..16u8 {
        for x in 0..16u8 {
            let column = runtime
                .natural_column(-16 + i32::from(x), -16 + i32::from(z))
                .unwrap();
            assert_ne!(base.get_block(x, column.ground_y, z), Some(runtime.air));
            if let Some(surface) = column.water_surface_y {
                for y in column.ground_y + 1..=surface {
                    assert_eq!(base.get_block(x, y, z), Some(water));
                }
                assert_eq!(base.get_block(x, surface + 1, z), Some(runtime.air));
            }
            for y in MIN_Y..MAX_Y {
                assert_eq!(base.get_block(x, y, z), pure.get_block(x, y, z));
            }
        }
    }
    assert_eq!(world.store().record_counts().unwrap(), Default::default());
    assert!(base.generated_entities().is_empty());
    assert_eq!(base.world_journal_lsn(), 0);
    assert!(!base.dirty);
    assert!(
        runtime
            .load_base(0, ChunkPos { x: 4, z: 0 })
            .unwrap()
            .is_none()
    );
    assert!(runtime.load_effective(1, position).is_err());
}

#[test]
fn native_provider_reopen_composes_edits_and_preserves_imported_air() {
    let directory = tempfile::tempdir().unwrap();
    let definition = definition();
    let blocks = registry();
    let items = Arc::new(mc_data::items::solaris_required_items());
    let position = ChunkPos { x: -1, z: -1 };
    let imported_position = ChunkPos { x: 1, z: 1 };
    let air = state(&blocks, "minecraft:air");
    let gold = state(&blocks, "minecraft:gold_block");
    {
        let world = Arc::new(
            prepare(
                directory.path(),
                &definition,
                definition.input().domain,
                PrepareOptions::default(),
            )
            .unwrap(),
        );
        let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None)
            .unwrap()
            .with_chest_loot(ChestLootCatalog::new(), items.clone());
        let base = runtime.load_base(0, position).unwrap().unwrap();
        let mut effective = base.clone();
        effective.set_block(3, MIN_Y + 4, 5, gold).unwrap();
        world
            .store()
            .apply_offline_snapshot(0, &base, &mut effective, &blocks, &items)
            .unwrap();
        let imported = Chunk::empty(
            imported_position,
            air,
            Identifier::parse("minecraft:plains").unwrap(),
        );
        world
            .store()
            .import_chunk(0, &imported, &blocks, &items)
            .unwrap();
    }
    let world = Arc::new(
        PreparedWorld::open(
            directory.path(),
            Some(&definition),
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None)
        .unwrap()
        .with_chest_loot(ChestLootCatalog::new(), items);
    let base = runtime.load_base(0, position).unwrap().unwrap();
    let effective = runtime.load_effective(0, position).unwrap().unwrap();
    assert_ne!(base.get_block(3, MIN_Y + 4, 5), Some(gold));
    assert_eq!(effective.get_block(3, MIN_Y + 4, 5), Some(gold));
    assert_eq!(
        effective.get_block(4, MIN_Y + 4, 5),
        base.get_block(4, MIN_Y + 4, 5)
    );
    let imported = runtime
        .load_effective(0, imported_position)
        .unwrap()
        .unwrap();
    for y in MIN_Y..MAX_Y {
        assert_eq!(imported.get_block(8, y, 8), Some(air));
    }
}

#[test]
fn spawn_refuses_insufficient_playable_clearance_and_fixture_roots() {
    let directory = tempfile::tempdir().unwrap();
    let definition = definition();
    let world = Arc::new(
        prepare(
            directory.path(),
            &definition,
            definition.input().domain,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let blocks = registry();
    let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None).unwrap();
    let source = runtime.base_source();
    let small = PlayableBounds {
        bounds: PlaneBounds {
            min_x: 0,
            min_z: 0,
            max_x_exclusive: 16,
            max_z_exclusive: 16,
        },
        operating_revision: 1,
    };
    assert!(matches!(
        source.locate_safe_spawn(small),
        Err(PrepareError::Invalid(
            "playable bounds too small for safe land spawn clearance"
        ))
    ));
    let fixture_directory = tempfile::tempdir().unwrap();
    let fixture = DefinitionInput::basin_fixture(42).validate().unwrap();
    let fixture_world = Arc::new(
        prepare(
            fixture_directory.path(),
            &fixture,
            fixture.input().domain,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    assert!(matches!(
        EarthRuntime::new(fixture_world, blocks, None),
        Err(PrepareError::Invalid(
            "live Earth runtime requires a prepared physical plane root"
        ))
    ));
}

#[test]
fn finite_boundary_halo_denies_only_uncovered_chunks_after_import_resolution() {
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
    let world = Arc::new(
        prepare(
            directory.path(),
            &definition,
            playable,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let blocks = registry();
    let items = Arc::new(mc_data::items::solaris_required_items());
    let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None)
        .unwrap()
        .with_chest_loot(ChestLootCatalog::new(), Arc::clone(&items));
    let air = state(&blocks, "minecraft:air");
    let imported_position = ChunkPos { x: 3, z: 0 };
    let mut imported = Chunk::empty(
        imported_position,
        air,
        Identifier::parse("minecraft:plains").unwrap(),
    );
    let gold = state(&blocks, "minecraft:gold_block");
    imported.set_block(5, MIN_Y + 4, 7, gold).unwrap();
    world
        .store()
        .import_chunk(0, &imported, &blocks, &items)
        .unwrap();

    // A valid playable boundary can admit its whole view/light halo. Missing
    // physical coverage is absence, not a fatal procedural preparation error.
    for z in -4..=3 {
        for x in -4..=3 {
            let position = ChunkPos { x, z };
            let base = runtime.load_base(0, position).unwrap();
            if position == imported_position {
                assert_eq!(base.unwrap().get_block(5, MIN_Y + 4, 7), Some(gold));
            } else if !(-3..=2).contains(&x) || !(-3..=2).contains(&z) {
                assert!(base.is_none(), "uncovered halo {position:?}");
                assert!(runtime.load_effective(0, position).unwrap().is_none());
            } else {
                assert!(base.is_some(), "overlapping physical chunk {position:?}");
            }
        }
    }
    for (chunk_x, valid_x, padding_x, world_x) in [(2, 0, 1, 32), (-3, 15, 14, -33)] {
        let position = ChunkPos { x: chunk_x, z: 0 };
        let base = runtime.load_base(0, position).unwrap().unwrap();
        let column = runtime.natural_column(world_x, 0).unwrap();
        assert_ne!(base.get_block(valid_x, column.ground_y, 0), Some(air));
        for y in MIN_Y..MAX_Y {
            assert_eq!(base.get_block(padding_x, y, 0), Some(air));
        }
    }
    let restored_import = runtime
        .load_effective(0, imported_position)
        .unwrap()
        .unwrap();
    assert_eq!(restored_import.get_block(5, MIN_Y + 4, 7), Some(gold));
    assert_eq!(restored_import.get_block(6, MIN_Y + 4, 7), Some(air));
    assert_eq!(world.store().record_counts().unwrap().imported_chunks, 1);
}

#[test]
fn prepared_natural_consumer_reads_exact_base_without_native_activation() {
    let directory = tempfile::tempdir().unwrap();
    let definition = definition();
    let world = Arc::new(
        prepare(
            directory.path(),
            &definition,
            definition.input().domain,
            PrepareOptions::default(),
        )
        .unwrap(),
    );
    let blocks = registry();
    let runtime = EarthRuntime::new(Arc::clone(&world), Arc::clone(&blocks), None).unwrap();
    let source = runtime.natural_source().unwrap();
    assert_eq!(source.seed(), definition.input().seed);
    for (x, z) in [(-64, -64), (63, 63), (0, 0)] {
        assert!(source.contains_column(x, z));
    }
    for (x, z) in [(-65, 0), (64, 0), (0, -65), (0, 64)] {
        assert!(!source.contains_column(x, z));
    }
    let positions = [(-16, -16), (-1, -1), (0, 0), (63, 63)];
    let facts = source.prepare_columns(&positions).unwrap();
    assert_eq!(facts.len(), positions.len());
    for (fact, &(x, z)) in facts.iter().zip(&positions) {
        let natural = runtime.natural_column(x, z).unwrap();
        assert_eq!(fact.position, (x, z));
        assert_eq!(fact.ground_y, natural.ground_y);
        assert_eq!(fact.biome, natural_biome(&natural));
        assert_eq!(fact.water_surface_y, natural.water_surface_y);
        assert_eq!(fact.protected, natural.protected);
        assert_eq!(
            fact.stable_ground,
            !natural.geology_ecology.ecology.wet && natural.geology_ecology.ecology.slope <= 0.7,
        );
        let base = runtime
            .load_base(
                0,
                ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                },
            )
            .unwrap()
            .unwrap();
        let (lx, lz) = (x.rem_euclid(16) as u8, z.rem_euclid(16) as u8);
        assert_ne!(base.get_block(lx, fact.ground_y, lz), Some(runtime.air));
        if let Some(surface) = fact.water_surface_y {
            for y in fact.ground_y + 1..=surface {
                assert_eq!(
                    base.get_block(lx, y, lz),
                    Some(state(&blocks, "minecraft:water"))
                );
            }
        }
    }
    assert!(source.prepare_columns(&[(64, 0)]).is_err());
    assert!(
        source
            .prepare_village_sites((-4, -4), (3, 3))
            .unwrap()
            .is_empty()
    );
    assert_eq!(world.store().record_counts().unwrap(), Default::default());
    assert_eq!(runtime.identity(), world.identity());
}

#[test]
fn prepared_site_consumer_reports_only_villages_written_into_exact_base() {
    use crate::structures::TemplateBlock;
    use crate::village::placement::RandomSpreadPlacement;
    let definition = definition();
    let built = WorldPlan::build(&definition).unwrap();
    let mut plane = built.plane().unwrap().clone();
    // A deterministic flat continental physical fixture, independent of biome
    // classification: the consumer invariant here is natural water protection.
    plane.elevation_m.fill(40.0);
    plane.ocean.fill(false);
    plane.ocean_seeds.clear();
    plane.tectonics.elevation_m.fill(40.0);
    plane.tectonics.erodibility.fill(0.8);
    plane.tectonics.boundaries.clear();
    plane.tectonics.structures.clear();
    plane.tectonics.provinces.clear();
    for crust in &mut plane.tectonics.crust {
        crust.continental_fraction = 1.0;
    }
    plane.climate_forcing.temperature_c.fill([25.0; 4]);
    plane.climate_forcing.precipitation_mm.fill([80.0; 4]);
    plane.climate_forcing.potential_et_mm.fill([100.0; 4]);
    plane.hydrology.downstream.fill(None);
    plane.hydrology.discharge_m3.fill([0.0; 4]);
    plane.hydrology.water_level_m.fill(None);
    plane.hydrology.seasonal_water_level_m.fill([None; 4]);
    plane.water_body_owner.fill([None; 4]);
    plane.basin_owner.fill(0);
    plane.runoff.soil_storage_m3.fill([0.0; 4]);
    plane.runoff.snow_storage_m3.fill([0.0; 4]);
    plane.runoff.snowmelt_m3.fill([0.0; 4]);
    plane.runoff.baseflow_m3.fill([0.0; 4]);
    for pass in &mut plane.erosion_passes {
        pass.source_m3.fill([0.0; 2]);
        pass.deposited_m3.fill([0.0; 2]);
    }
    plane.region_geology =
        crate::earth::region_geology::GeologyContext::build(&plane.grid, &plane.tectonics).unwrap();
    let root = WorldPlan::Plane(Box::new(plane));
    let identity = WorldIdentity::derive(&definition, [17; 32], [23; 32]).unwrap();
    let marker = state(&registry(), "minecraft:diamond_block");
    let (villages, blocks) = crate::village_plan_source_tests::synthetic_source_with_template(
        42,
        [1, 2, 1],
        RandomSpreadPlacement {
            spacing: 1,
            separation: 0,
            salt: 10_387_312,
            triangular: false,
        },
        Vec::new(),
        vec![TemplateBlock {
            pos: [0, 1, 0],
            state: marker,
        }],
    );
    struct ProtectionFixtureTags;
    impl crate::vanilla_features::BlockTagIndex for ProtectionFixtureTags {
        fn block_in_tag(&self, _: &Identifier, _: &Identifier) -> bool {
            false
        }
    }
    impl crate::vanilla_features::BiomeTagIndex for ProtectionFixtureTags {
        fn biome_in_tag(&self, _: &Identifier, _: &Identifier) -> bool {
            true
        }
    }
    let mut closure = villages.closure().clone();
    for structure in &mut closure.structures {
        structure.spec.biomes = mc_data::village_data::BiomeSet::Tag(
            Identifier::parse("test:protection_fixture_all_biomes").unwrap(),
        );
    }
    let villages = Arc::new(
        VillagePlanSource::new(
            Arc::new(closure),
            Arc::new(crate::village::VillageDecor::empty()),
            42,
            Arc::clone(&blocks),
            Arc::new(ProtectionFixtureTags),
            Arc::new(ProtectionFixtureTags),
        )
        .unwrap(),
    );
    let loot = ChestLootCatalog::new();
    let items = mc_data::items::solaris_required_items();
    let cache = EarthSourceCache::default();
    let base = EarthBaseSource::new(
        &definition,
        identity,
        &root,
        &blocks,
        Some(&villages),
        StructureLoot {
            seed: 42,
            catalog: &loot,
            items: &items,
        },
        &cache,
    )
    .unwrap();
    let consumer: &dyn NativeNaturalSource = &base;
    let sites = consumer.prepare_village_sites((0, 0), (0, 0)).unwrap();
    assert_eq!(sites.len(), 1, "flat dry start must be accepted");
    let site = &sites[0];
    assert_eq!(site.start_chunk, (0, 0));
    assert_eq!(site.pieces.len(), 1);
    let piece = &site.pieces[0];
    let position = ChunkPos {
        x: piece.position.x.div_euclid(16),
        z: piece.position.z.div_euclid(16),
    };
    let chunk = base.materialize_chunk(position).unwrap();
    assert_eq!(
        chunk.get_block(
            piece.position.x.rem_euclid(16) as u8,
            piece.position.y + 1,
            piece.position.z.rem_euclid(16) as u8,
        ),
        Some(marker),
        "site consumer and block consumer must use the same accepted plan",
    );
    let columns = consumer
        .prepare_columns(&[(piece.position.x, piece.position.z)])
        .unwrap();
    assert!(columns[0].stable_ground);
    assert!(!columns[0].protected);
    assert_eq!(columns[0].water_surface_y, None);
    assert!(
        consumer
            .prepare_village_sites((4, 0), (4, 0))
            .unwrap()
            .is_empty()
    );
    assert!(chunk.generated_entities().is_empty());
    assert_eq!(chunk.world_journal_lsn(), 0);
    assert!(!chunk.dirty);

    // An earlier-season wet footprint is protected even when its final-season
    // rendered terrain is dry. The biome-neutral fixture isolates protection:
    // the generic site and exact chunk consumers must reject the same raw plan.
    let mut seasonal_plane = root.plane().unwrap().clone();
    seasonal_plane
        .hydrology
        .seasonal_water_level_m
        .fill([Some(41.0), None, None, None]);
    seasonal_plane
        .water_body_owner
        .fill([Some(0), None, None, None]);
    let seasonal_root = WorldPlan::Plane(Box::new(seasonal_plane));
    let seasonal_identity = WorldIdentity::derive(&definition, [31; 32], [37; 32]).unwrap();
    let seasonal_cache = EarthSourceCache::default();
    let seasonal_base = EarthBaseSource::new(
        &definition,
        seasonal_identity,
        &seasonal_root,
        &blocks,
        Some(&villages),
        StructureLoot {
            seed: 42,
            catalog: &loot,
            items: &items,
        },
        &seasonal_cache,
    )
    .unwrap();
    let facts = seasonal_base
        .prepare_columns(&[(piece.position.x, piece.position.z)])
        .unwrap();
    assert!(facts[0].protected);
    assert_eq!(facts[0].water_surface_y, None);
    let raw_plan = seasonal_base
        .with_village_facts(|height, biome| villages.plan_for_start_chunk(0, 0, height, biome))
        .unwrap();
    assert!(
        raw_plan.is_some(),
        "biome gate must admit the raw candidate"
    );
    assert!(
        seasonal_base
            .prepare_village_sites((0, 0), (0, 0))
            .unwrap()
            .is_empty()
    );
    let seasonal_chunk = seasonal_base.materialize_chunk(position).unwrap();
    assert_ne!(
        seasonal_chunk.get_block(
            piece.position.x.rem_euclid(16) as u8,
            piece.position.y + 1,
            piece.position.z.rem_euclid(16) as u8,
        ),
        Some(marker),
        "a rejected site's structure must not enter the exact base",
    );
}

#[test]
fn thin_soil_on_flat_low_coastal_land_is_not_a_peak_biome() {
    let definition = definition();
    let root = WorldPlan::build(&definition).unwrap();
    let identity = WorldIdentity::derive(&definition, [43; 32], [47; 32]).unwrap();
    let planner = RegionPlanner::new(root.plane().unwrap(), identity);
    let node = planner.leaf_at(0.0, 0.0).unwrap();
    let mut column = planner.prepare(node).unwrap().sample(0.0, 0.0).unwrap();
    column.water = WaterPlan::None;
    column.water_surface_y = None;
    column.ground_m = 20.0;
    column.geology_ecology.ecology.temperature_c = [15.0; 4];
    column.geology_ecology.ecology.moisture_fraction = [0.6; 4];
    column.geology_ecology.ecology.water_deficit_mm = [0.0; 4];
    column.geology_ecology.ecology.soil_depth_m = 0.1;
    column.geology_ecology.ecology.rock_exposure = 0.8;
    column.geology_ecology.ecology.slope = 0.012;
    let peaks = Identifier::parse("minecraft:stony_peaks").unwrap();
    assert_ne!(
        natural_biome(&column),
        peaks,
        "thin soil and rock exposure are not mountain relief on a flat low bank"
    );
    column.ground_m = 2_000.0;
    assert_ne!(
        natural_biome(&column),
        peaks,
        "a high flat plateau must not become peaks from thin soil alone"
    );
    column.geology_ecology.ecology.slope = 0.2;
    assert_eq!(natural_biome(&column), peaks);
    column.ground_m = 20.0;
    assert_ne!(
        natural_biome(&column),
        peaks,
        "steep low coastal rock does not establish an alpine altitude belt"
    );
}
