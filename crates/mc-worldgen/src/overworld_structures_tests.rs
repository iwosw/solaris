use super::*;
use mc_world::{ChunkGenerator, ChunkPos};
use std::sync::Arc;

fn registry() -> BlockRegistry {
    BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report()).unwrap()
}
fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla")
}

#[test]
fn stronghold_portal_room_has_twelve_inward_frames_spawner_and_loot() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let frame = id("minecraft:end_portal_frame");
    let frames: Vec<_> = template
        .blocks()
        .iter()
        .filter(|b| registry.by_id(b.state).unwrap().block.id == frame)
        .collect();
    assert_eq!(frames.len(), 12);
    for block in frames {
        let state = registry.by_id(block.state).unwrap();
        let facing = state
            .properties
            .iter()
            .find(|(k, _)| k == "facing")
            .unwrap()
            .1
            .as_str();
        assert_eq!(
            facing,
            if block.pos[0] == 5 {
                "east"
            } else if block.pos[0] == 9 {
                "west"
            } else if block.pos[2] == 5 {
                "south"
            } else {
                "north"
            }
        );
    }
    assert_eq!(template.chests().len(), 2);
    assert_eq!(template.block_entities().len(), 1);
    let pos = BlockPos {
        x: -49,
        y: -20,
        z: 70,
    };
    let bytes = template.block_entities()[0].at_world_position(pos);
    let nbt = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
    let Tag::Compound(fields) = nbt else {
        panic!("compound")
    };
    assert!(fields.contains(&("x".into(), Tag::Int(-49))));
    assert!(fields.contains(&(
        "SpawnData".into(),
        Tag::Compound(vec![(
            "entity".into(),
            Tag::Compound(vec![(
                "id".into(),
                Tag::String("minecraft:silverfish".into())
            )])
        )])
    )));
}

#[test]
fn stronghold_clipped_paste_matches_whole_plan_and_preserves_entities() {
    let registry = registry();
    let template = stronghold(&registry).unwrap();
    let origin = BlockPos {
        x: -16,
        y: -30,
        z: -32,
    };
    let mut chunks = BTreeMap::new();
    for x in -1..=2 {
        for z in -2..=0 {
            let mut chunk = Chunk::empty(
                ChunkPos { x, z },
                state(&registry, "minecraft:stone").unwrap(),
                id("minecraft:plains"),
            );
            paste_template(&mut chunk, &template, origin, 42);
            chunks.insert((x, z), chunk);
        }
    }
    for block in template.blocks() {
        let (x, y, z) = (
            origin.x + block.pos[0],
            origin.y + block.pos[1],
            origin.z + block.pos[2],
        );
        let chunk = &chunks[&(x.div_euclid(16), z.div_euclid(16))];
        assert_eq!(
            chunk.get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8),
            Some(block.state)
        );
    }
    assert_eq!(chunks.values().map(|c| c.chests.len()).sum::<usize>(), 2);
    assert_eq!(
        chunks
            .values()
            .map(|c| c.block_entities.len())
            .sum::<usize>(),
        1
    );
}

#[test]
fn vanilla_catalog_loads_and_located_stronghold_generates_portal() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = Arc::new(registry());
    let mut generator = crate::TerrainGenerator::new(42, Arc::clone(&registry));
    generator.load_vanilla_worldgen(&root()).unwrap();
    let structures = generator.structure_sites_in_region((-180, -180), (180, 180));
    let site = structures
        .iter()
        .find(|s| s.structure == id("minecraft:stronghold"))
        .unwrap();
    let chunk = generator.generate(ChunkPos {
        x: site.start_chunk.0,
        z: site.start_chunk.1,
    });
    let expected = registry
        .by_name_and_props(
            &id("minecraft:end_portal_frame"),
            &[
                ("eye".into(), "false".into()),
                ("facing".into(), "south".into()),
            ],
        )
        .unwrap();
    assert_eq!(chunk.get_block(6, site.position.y + 3, 5), Some(expected));
    assert!(chunk.block_entities.contains_key(&BlockPos {
        x: site.position.x + 7,
        y: site.position.y + 1,
        z: site.position.z + 14
    }));
    assert!(
        mc_world::structure_starts::read(&chunk.extras, chunk.pos, false)
            .unwrap()
            .iter()
            .any(|saved| saved == site)
    );
}

#[test]
fn mineshaft_has_walkable_crossing_rails_cave_spiders_and_loot() {
    let registry = registry();
    let template = mineshaft(&registry).unwrap();
    assert_eq!(
        template.chests()[0].loot_table,
        Some(id("minecraft:chests/abandoned_mineshaft"))
    );
    assert_eq!(template.block_entities().len(), 1);
    assert!(template.blocks().iter().any(|b| b.pos==[24,1,24] && b.state==state(&registry,"minecraft:rail").unwrap()));
}

#[test]
fn vanilla_shipwreck_markers_become_loot_not_authoring_blocks() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = registry();
    let template = StructureTemplate::from_nbt_file(
        root().join("data/minecraft/structure/shipwreck/with_mast.nbt"),
        &registry,
    )
    .unwrap();
    let template = clean_template(template, "shipwreck", &registry).unwrap();
    assert!(!template.blocks().iter().any(|block| {
        registry.by_id(block.state).unwrap().block.id == id("minecraft:structure_block")
    }));
    assert!(
        template
            .chests()
            .iter()
            .all(|chest| chest.loot_table.is_some())
    );
    assert!(
        template
            .chests()
            .iter()
            .any(|chest| chest.loot_table == Some(id("minecraft:chests/shipwreck_map")))
    );
}

#[test]
fn classic_catalog_contains_every_family_with_interactive_contents() {
    if !root().exists() {
        eprintln!("SKIP local vanilla assets unavailable");
        return;
    }
    let registry = registry();
    for name in [
        "mineshaft",
        "mineshaft_mesa",
        "igloo",
        "shipwreck",
        "shipwreck_beached",
        "ocean_ruin_warm",
        "ocean_ruin_cold",
        "pillager_outpost",
        "desert_pyramid",
        "jungle_pyramid",
        "swamp_hut",
        "buried_treasure",
        "monument",
        "mansion",
        "ruined_portal",
    ] {
        let (templates, _) = catalog::templates(&root(), name, &registry).unwrap();
        assert!(!templates.is_empty(), "{name}");
        for t in templates {
            assert!(!t.blocks().is_empty(), "{name}");
            assert!(
                t.blocks()
                    .iter()
                    .all(|b| registry.by_id(b.state).unwrap().block.id
                        != id("minecraft:structure_block")),
                "{name}"
            );
            assert!(t.chests().iter().all(|c| c.loot_table.is_some()), "{name}");
        }
    }
    let (temples, _) = catalog::templates(&root(), "jungle_pyramid", &registry).unwrap();
    assert_eq!(temples[0].block_entities().len(), 2);
    for entity in temples[0].block_entities() {
        assert!(
            entity
                .nbt
                .contains(&("id".into(), Tag::String("minecraft:dispenser".into())))
        );
        assert!(entity.nbt.iter().any(|(key, value)| key == "Items"
            && matches!(value,Tag::List(list) if !list.elements.is_empty())));
    }
    let (monuments, _) = catalog::templates(&root(), "monument", &registry).unwrap();
    assert_eq!(
        monuments[0]
            .entities()
            .iter()
            .filter(|e| e.entity_type == "minecraft:elder_guardian" && e.persistent)
            .count(),
        3
    );
    let (igloos, _) = catalog::templates(&root(), "igloo", &registry).unwrap();
    assert_eq!(igloos.len(), 2);
    assert!(
        igloos[1].bounds().0[1] < 0,
        "basement participates in saved bounds"
    );
}

#[test]
fn generated_shipwreck_map_chest_preserves_unopened_table_and_seed() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    assert!(root.exists(), "local vanilla assets required");
    let registry = registry();
    let (templates, _) = catalog::templates(&root, "shipwreck", &registry).unwrap();
    let (template, chest) = templates
        .iter()
        .find_map(|template| {
            template
                .chests()
                .iter()
                .find(|chest| {
                    chest
                        .loot_table
                        .as_ref()
                        .is_some_and(|id| id.as_str() == "minecraft:chests/shipwreck_map")
                })
                .map(|chest| (template, chest))
        })
        .expect("shipwreck map chest asset");
    let position = BlockPos {
        x: chest.pos[0],
        y: 64 + chest.pos[1],
        z: chest.pos[2],
    };
    let mut chunk = Chunk::empty(
        ChunkPos {
            x: position.x.div_euclid(16),
            z: position.z.div_euclid(16),
        },
        state(&registry, "minecraft:air").unwrap(),
        id("minecraft:ocean"),
    );
    paste_template(&mut chunk, template, BlockPos { x: 0, y: 64, z: 0 }, 42);
    let generated = &chunk.chests[&position];
    assert_eq!(
        generated.loot_table,
        Some(id("minecraft:chests/shipwreck_map"))
    );
    assert_eq!(
        generated.loot_seed,
        (hash(42, position.x, position.z) ^ position.y as u64) as i64
    );
    assert!(
        generated.slots.iter().all(|slot| slot.count == 0),
        "unopened, not prematurely rolled without map context"
    );
}

#[test]
fn igloo_basement_has_continuous_ladder_and_pastes_north_of_start() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla");
    let registry = registry();
    let mut structures = OverworldStructures::load(&root, 42, &registry).unwrap();
    structures.strongholds.clear();
    structures.features.clear();
    structures
        .families
        .retain(|family| family.id.path() == "igloo");
    let family = &mut structures.families[0];
    let basement = family.templates.remove(1);
    for y in -22..0 {
        let block = basement
            .blocks()
            .iter()
            .find(|block| block.pos == [3, y, 5])
            .expect("continuous basement-to-trapdoor shaft");
        assert_eq!(
            registry.by_id(block.state).unwrap().block.id.path(),
            "ladder"
        );
    }
    let (cx, cz) = family.placement.potential_chunk(42, 0, 0);
    family.templates = vec![basement.clone()];
    assert!(family.biomes.contains(&id("minecraft:snowy_plains")));
    let mut chunk = Chunk::empty(
        ChunkPos { x: cx, z: cz - 1 },
        state(&registry, "minecraft:air").unwrap(),
        id("minecraft:snowy_plains"),
    );
    structures.paste(
        &mut chunk,
        |_, _| 64,
        |_, _, _| id("minecraft:snowy_plains"),
    );
    let mut checked = 0;
    for block in basement.blocks().iter().filter(|block| block.pos[2] < 0) {
        assert_eq!(
            chunk.get_block(
                block.pos[0] as u8,
                65 + block.pos[1],
                (16 + block.pos[2]) as u8
            ),
            Some(block.state)
        );
        checked += 1;
    }
    assert!(
        checked > 0,
        "basement crosses the negative-Z chunk boundary"
    );
}

#[test]
fn outpost_exclusion_rejects_nearby_village_placement_before_terrain_sampling() {
    let registry = registry();
    let mut structures = OverworldStructures::load(&root(), 42, &registry).unwrap();
    structures.strongholds.clear();
    structures
        .families
        .retain(|family| family.id.path() == "pillager_outpost");
    let family = &mut structures.families[0];
    assert_eq!(
        family.exclusion.unwrap().1,
        10,
        "local vanilla exclusion radius"
    );
    let grid = RandomSpreadPlacement {
        spacing: 100,
        separation: 99,
        salt: 1,
        triangular: false,
    };
    family.placement = grid;
    family.frequency = 1.0;
    family.exclusion = Some((grid, 10));
    assert!(
        structures
            .sites(
                (0, 0),
                (0, 0),
                |_, _| panic!("excluded start must not sample terrain"),
                |_, _, _| id("minecraft:plains")
            )
            .is_empty()
    );
    structures.families[0].exclusion = None;
    assert_eq!(
        structures
            .sites((0, 0), (0, 0), |_, _| 64, |_, _, _| id("minecraft:plains"))
            .len(),
        1
    );
    assert!(near_placement(42, grid, 10, -90, 190));
    assert!(!near_placement(42, grid, 10, -89, 189));
}
