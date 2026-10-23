use std::sync::Arc;

use mc_data::Identifier;
use mc_data::village_data::{HeightProviderSpec, VerticalAnchor};
use mc_world::{BlockRegistry, ChunkGenerator, ChunkPos};

use super::TerrainGenerator;
use crate::vanilla_features::{BlockSemantics, CacheTags, LegacyRandom};
use crate::village::plan_source::VillagePlanSource;
use crate::village::solver::sample_start_height;
use crate::village::{VillageDecor, load_village_closure};

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

#[test]
fn modern_jigsaw_variable_heights_are_bounded_and_deterministic() {
    for provider in [
        HeightProviderSpec::Uniform {
            min_inclusive: VerticalAnchor::Absolute(-40),
            max_inclusive: VerticalAnchor::Absolute(-20),
        },
        HeightProviderSpec::Trapezoid {
            min_inclusive: VerticalAnchor::AboveBottom(24),
            max_inclusive: VerticalAnchor::BelowTop(339),
            plateau: 4,
        },
    ] {
        let sample = || {
            let mut random = LegacyRandom::new(4242);
            (0..128)
                .map(|_| sample_start_height(provider, -64, 320, &mut random))
                .collect::<Vec<_>>()
        };
        let heights = sample();
        assert_eq!(heights, sample());
        assert!(heights.iter().all(|y| (-40..=-20).contains(y)));
        assert!(heights.iter().any(|y| *y != heights[0]));
    }
    assert_eq!(
        sample_start_height(
            HeightProviderSpec::Constant(VerticalAnchor::BelowTop(10)),
            0,
            128,
            &mut LegacyRandom::new(0)
        ),
        117
    );
}

/// Real vanilla JSON/NBT integration, not a substituted structure fixture.
#[test]
#[ignore = "requires the local licensed vanilla content cache"]
fn modern_jigsaw_seeded_structures_generate_with_bounded_work() {
    let cache = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let vanilla = Arc::new(mc_data::load(cache.clone()).unwrap());
    let tags = Arc::new(mc_data::tags::load(&cache, &vanilla).unwrap());
    let tags = Arc::new(CacheTags::new(blocks.clone(), vanilla, tags));
    let seed = 4242;
    for (set, structure, spacing, separation, salt) in [
        ("ancient_cities", "ancient_city", 24, 8, 20083232),
        ("trail_ruins", "trail_ruins", 34, 8, 83469867),
        ("trial_chambers", "trial_chambers", 34, 12, 94251327),
    ] {
        let closure = Arc::new(
            load_village_closure(&cache, &id(&format!("minecraft:{set}")), &blocks).unwrap(),
        );
        assert_eq!(
            (
                closure.placement.spacing,
                closure.placement.separation,
                closure.placement.salt
            ),
            (spacing, separation, salt)
        );
        let decor = Arc::new(
            VillageDecor::compile(
                &cache,
                &closure,
                &BlockSemantics::new(&blocks, tags.as_ref()),
            )
            .unwrap(),
        );
        let source = Arc::new(
            VillagePlanSource::new(
                closure.clone(),
                decor,
                seed,
                blocks.clone(),
                tags.clone(),
                tags.clone(),
            )
            .unwrap(),
        );
        let mut terrain =
            TerrainGenerator::new(seed, blocks.clone()).with_jigsaw_source(source.clone());
        terrain.load_vanilla_worldgen(&cache).unwrap();
        let surface = |x, z| terrain.first_free_height(x, z);
        let biome = |x, y, z| terrain.noise_biome(x, y, z);
        let mut found = None;
        'search: for gz in -16..=16 {
            for gx in -16..=16 {
                let (x, z) = closure
                    .placement
                    .potential_chunk(seed, gx * spacing, gz * spacing);
                if let Some(plan) = source.plan_for_start_chunk(x, z, &surface, &biome) {
                    found = Some(plan);
                    break 'search;
                }
            }
        }
        let plan = found
            .unwrap_or_else(|| panic!("{structure}: no real-biome start in seeded grid -16..16"));
        let assembly = plan.assembly();
        let spec = &closure.structures[0].spec;
        assert_eq!(assembly.structure, id(&format!("minecraft:{structure}")));
        assert!(assembly.pieces.len() > 1);
        assert!(
            assembly
                .pieces
                .iter()
                .all(|piece| piece.depth <= spec.size + 1)
        );
        assert!(assembly.pieces.iter().all(|piece| piece.bounds_min.y
            >= -64 + spec.dimension_padding.bottom
            && piece.bounds_max.y < 320 - spec.dimension_padding.top));
        assert!(
            assembly.candidate_attempts < 200_000,
            "unexpected candidate explosion"
        );
        assert!(assembly.pieces.len() < 2_000, "unexpected piece explosion");
        let (x, z) = plan.start_chunk();
        let repeat = source.plan_for_start_chunk(x, z, &surface, &biome).unwrap();
        assert_eq!(assembly, repeat.assembly());
        if structure == "ancient_city" {
            assert_eq!(
                biome(assembly.origin.x, assembly.origin.y, assembly.origin.z),
                id("minecraft:deep_dark")
            );
        }
        let chunk = terrain.generate(ChunkPos { x, z });
        let again = terrain.generate(ChunkPos { x, z });
        assert_eq!(chunk.block_entities, again.block_entities);
        assert_eq!(chunk.chests, again.chests);
        let starts = mc_world::structure_starts::read(&chunk.extras, chunk.pos, false).unwrap();
        let expected = source.structure_sites_in_region((x, z), (x, z), &surface, &biome);
        assert_eq!(
            starts, expected,
            "saved start agrees with generation lookup"
        );
        assert_eq!(starts[0].position, assembly.origin);
        for wanted in match structure {
            "trial_chambers" => &["trial_spawner", "vault"][..],
            "ancient_city" => &["chest"][..],
            _ => &[][..],
        } {
            let (position, payload, table) = plan
                .pieces()
                .iter()
                .find_map(|piece| {
                    let crate::village::plan_source::PlanElement::Single { template, .. } =
                        &piece.element
                    else {
                        return None;
                    };
                    let template = closure.piece(template).unwrap();
                    let (local, payload, table) = if *wanted == "chest" {
                        let chest = template
                            .chests()
                            .iter()
                            .find(|chest| chest.loot_table.is_some())?;
                        (chest.pos, None, chest.loot_table.clone())
                    } else {
                        let entity = template.block_entities().iter().find(|entity| {
                            blocks
                                .by_id(entity.state)
                                .is_some_and(|state| state.block.id.path() == *wanted)
                        })?;
                        (entity.pos, Some(entity), None)
                    };
                    let p = piece.rotation.transform(local);
                    let position = mc_world::BlockPos {
                        x: piece.position.x + p[0],
                        y: piece.position.y + p[1],
                        z: piece.position.z + p[2],
                    };
                    Some((position, payload, table))
                })
                .unwrap_or_else(|| panic!("{structure}: no authored {wanted} in seeded plan"));
            let chunk = terrain.generate(ChunkPos {
                x: position.x.div_euclid(16),
                z: position.z.div_euclid(16),
            });
            let state = chunk
                .get_block(
                    position.x.rem_euclid(16) as u8,
                    position.y,
                    position.z.rem_euclid(16) as u8,
                )
                .unwrap();
            assert_eq!(blocks.by_id(state).unwrap().block.id.path(), *wanted);
            if let Some(payload) = payload {
                assert_eq!(
                    chunk.block_entities.get(&position),
                    Some(&payload.at_world_position(position))
                );
            } else {
                assert_eq!(
                    chunk.chests[&position].loot_table, table,
                    "unopened real loot table is retained"
                );
            }
        }
        let paste = |cx, cz| {
            let plans = source.plans_for_chunk(cx, cz, &surface, &biome).unwrap();
            let mut chunk = mc_world::Chunk::empty(
                ChunkPos { x: cx, z: cz },
                terrain.air,
                id("minecraft:plains"),
            );
            let written = source
                .write_plans(&mut chunk, &plans, &mut [false; 256], &surface, terrain.air)
                .unwrap();
            (chunk, written)
        };
        let (_, pasted_blocks) = paste(x, z);
        assert!(pasted_blocks > 0);
        if structure == "trail_ruins" {
            let piece = plan.pieces().iter().find(|piece| matches!(&piece.element,
                crate::village::plan_source::PlanElement::Single { processors, .. }
                if processors.iter().any(|p| matches!(p, mc_data::village_data::StructureProcessorSpec::Capped { .. })))).unwrap();
            let mut brushable = None;
            'chunks: for cz in piece.bounds_min.z.div_euclid(16)..=piece.bounds_max.z.div_euclid(16)
            {
                for cx in piece.bounds_min.x.div_euclid(16)..=piece.bounds_max.x.div_euclid(16) {
                    let (chunk, _) = paste(cx, cz);
                    if let Some((position, bytes)) =
                        chunk.block_entities.iter().find(|(position, _)| {
                            chunk
                                .get_block(
                                    position.x.rem_euclid(16) as u8,
                                    position.y,
                                    position.z.rem_euclid(16) as u8,
                                )
                                .and_then(|state| blocks.by_id(state))
                                .is_some_and(|state| state.block.id.path() == "suspicious_gravel")
                        })
                    {
                        brushable = Some((*position, bytes.clone()));
                        break 'chunks;
                    }
                }
            }
            let (position, payload) =
                brushable.expect("real trail-ruins processors produce archaeology loot");
            let actual = terrain.generate(ChunkPos {
                x: position.x.div_euclid(16),
                z: position.z.div_euclid(16),
            });
            assert_eq!(actual.block_entities.get(&position), Some(&payload));
        }
        let start = &assembly.pieces[0];
        let mut marker_checks = 0;
        let mut leaves = Vec::new();
        start.element.leaves(&mut leaves);
        for leaf in leaves {
            if let crate::village::solver::PlacedElement::Single { template, .. } = leaf {
                for block in closure.piece(template).unwrap().blocks() {
                    let p = start.rotation.transform(block.pos);
                    let wx = start.position.x + p[0];
                    let wy = start.position.y + p[1];
                    let wz = start.position.z + p[2];
                    if wx.div_euclid(16) == x && wz.div_euclid(16) == z {
                        assert_eq!(
                            chunk.get_block(wx.rem_euclid(16) as u8, wy, wz.rem_euclid(16) as u8),
                            again.get_block(wx.rem_euclid(16) as u8, wy, wz.rem_euclid(16) as u8)
                        );
                        marker_checks += 1;
                    }
                }
            }
        }
        assert!(marker_checks > 0);
        println!(
            "{structure} seed={seed} chunk=({x},{z}) pieces={} max_depth={} candidate_attempts={} start_chunk_template_cells={marker_checks} pasted_blocks={pasted_blocks} block_entities={} chests={}",
            assembly.pieces.len(),
            assembly.pieces.iter().map(|p| p.depth).max().unwrap(),
            assembly.candidate_attempts,
            chunk.block_entities.len(),
            chunk.chests.len()
        );
    }
}
