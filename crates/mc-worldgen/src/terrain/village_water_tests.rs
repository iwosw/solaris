//! Production-content regression for village floors on the owner's world seed.
use super::*;
use crate::vanilla_features::{BlockSemantics, CacheTags};
use crate::village::{VillageDecor, load_village_closure};
use mc_data::village_data::Projection;
use mc_world::BlockPos;

#[test]
fn owner_seed_village_floors_stay_above_water() {
    let root = std::env::var_os("SOLARIS_CONTENT_CACHE")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
        });
    if !root.join("data/minecraft/worldgen").is_dir() {
        println!(
            "SKIP owner_seed_village_floors_stay_above_water: no content cache at {}",
            root.display()
        );
        return;
    }
    let seed = 8674276330747006396;
    let blocks = Arc::new(
        BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap(),
    );
    let vanilla = mc_data::load(&root).unwrap();
    let tags = mc_data::tags::load(&root, &vanilla).unwrap();
    let tags = Arc::new(CacheTags::new(
        Arc::clone(&blocks),
        Arc::new(vanilla),
        Arc::new(tags),
    ));
    let closure = Arc::new(
        load_village_closure(
            &root,
            &Identifier::parse("minecraft:villages").unwrap(),
            &blocks,
        )
        .unwrap(),
    );
    let decor = Arc::new(
        VillageDecor::compile(
            &root,
            &closure,
            &BlockSemantics::new(&blocks, tags.as_ref()),
        )
        .unwrap(),
    );
    let source = Arc::new(
        VillagePlanSource::new(
            Arc::clone(&closure),
            decor,
            seed,
            Arc::clone(&blocks),
            tags.clone(),
            tags,
        )
        .unwrap(),
    );
    let mut generator = TerrainGenerator::new(seed, Arc::clone(&blocks));
    generator.load_vanilla_worldgen(&root).unwrap();
    let generator = generator.with_village_plans(Arc::clone(&source));
    // The piece writer's gravity projection must use the same water-inclusive
    // surface as assembly, without changing solid terrain height queries.
    let wet_column = (-282, 4);
    let solid_y = generator.base_surface_y(wet_column.0, wet_column.1);
    assert!(
        solid_y < SEA_LEVEL - 1,
        "fixture must sample an underwater column"
    );
    let mut heights = HeightContext::for_chunk(
        &generator,
        ChunkPos { x: -18, z: 0 },
        &[(wet_column.0, wet_column.1, solid_y)],
        None,
    );
    assert_eq!(
        heights.structure_surface(wet_column.0, wet_column.1),
        SEA_LEVEL - 1
    );
    assert_eq!(
        generator.first_free_height(wet_column.0, wet_column.1),
        SEA_LEVEL
    );
    let mut sunken_total = 0;
    let mut blocked_entrances = 0;
    let mut stairs_fixture = None;
    for chunk in [(16, 6), (-18, 0), (18, 69)] {
        let sites = generator.village_sites_in_region(chunk, chunk);
        assert_eq!(sites.len(), 1, "expected the owner's village at {chunk:?}");
        let plan = source
            .plan_for_start_chunk(
                chunk.0,
                chunk.1,
                &|x, z| generator.first_free_height(x, z),
                &|x, y, z| generator.noise_biome(x, y, z),
            )
            .unwrap();
        let mut rigid = 0;
        let mut sunken = 0;
        let mut entrances = 0;
        let mut blocked = 0;
        for piece in &plan.assembly().pieces {
            if piece.projection != Projection::Rigid {
                continue;
            }
            rigid += 1;
            // Ground level is the first walkable Y, one above the floor.
            let ground_y = piece.bounds_min.y + piece.ground_level_delta;
            if ground_y < SEA_LEVEL {
                sunken += 1;
                println!(
                    "sunken start={chunk:?} element={:?} min={:?} ground_y={ground_y}",
                    piece.element, piece.bounds_min
                );
            }
            let crate::village::solver::PlacedElement::Single { template, .. } = &piece.element
            else {
                continue;
            };
            for jigsaw in closure.piece(template).unwrap().jigsaws() {
                if jigsaw.name.as_str() != "minecraft:building_entrance" {
                    continue;
                }
                let local = piece.rotation.transform(jigsaw.pos);
                let [dx, _, dz] = piece.rotation.rotate_face(jigsaw.front).step();
                let step = BlockPos {
                    x: piece.position.x + local[0],
                    y: piece.position.y + local[1],
                    z: piece.position.z + local[2],
                };
                if stairs_fixture.is_none()
                    && blocks
                        .by_id(jigsaw.final_state)
                        .unwrap()
                        .block
                        .id
                        .path()
                        .ends_with("_stairs")
                {
                    stairs_fixture = Some((step, dx, dz));
                }
                entrances += 1;
                for (x, z) in [(step.x, step.z), (step.x + dx, step.z + dz)] {
                    let terrain_y = generator.surface_height(x, z);
                    if terrain_y >= step.y {
                        blocked += 1;
                        println!(
                            "blocked_entrance start={chunk:?} step={step:?} column=({x},{z}) terrain_y={terrain_y}"
                        );
                    }
                }
            }
        }
        println!(
            "water_census start={chunk:?} pieces={} rigid={rigid} sunken={sunken}",
            plan.assembly().pieces.len()
        );
        sunken_total += sunken;
        println!("entrance_census start={chunk:?} entrances={entrances} blocked_columns={blocked}");
        blocked_entrances += blocked;
    }
    assert_eq!(
        sunken_total, 0,
        "village floors must not be below the surrounding sea surface"
    );
    assert_eq!(
        blocked_entrances, 0,
        "terrain must leave the entrance step and its approach clear"
    );
    // Exercise a real house stair, not an animal pen's intentionally authored
    // fence. Road gravity must not put soil back across the cleared approach.
    let (step, dx, dz) = stairs_fixture.expect("the seed has a house entrance stair");
    println!("generated_entrance_fixture step={step:?} front=({dx},{dz})");
    let mut chunks = std::collections::BTreeMap::new();
    for (x, z, bottom) in [
        (step.x, step.z, step.y + 1),
        (step.x + dx, step.z + dz, step.y),
    ] {
        let chunk = chunks
            .entry((x.div_euclid(16), z.div_euclid(16)))
            .or_insert_with(|| {
                generator.generate(ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                })
            });
        for y in bottom..=step.y + 2 {
            assert_eq!(
                chunk.get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8),
                Some(generator.air),
                "entrance headroom must be clear at ({x},{y},{z})"
            );
        }
    }
}
