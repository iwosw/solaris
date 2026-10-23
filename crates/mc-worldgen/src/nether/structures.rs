use super::landscape::put;
use super::*;
use crate::structures::{StructureTemplate, TemplateBlock, TemplateBlockEntity, TemplateChest};
use crate::vanilla_features::{BiomeTagIndex, BlockSemantics, CacheTags};
use crate::village::closure::load_jigsaw_closure;
use crate::village::piece::rotate_state;
use crate::village::plan_source::{VillagePlanSet, VillagePlanSource};
use crate::village::{RandomSpreadPlacement, Rotation, VillageDecor};
use mc_data::village_data::{PlacementSpec, RandomSpreadType, VillageDataLoader};
use mc_nbt::Tag;
use mc_world::chunk::GeneratedStructureSite;
use mc_world::{BlockPos, ChestBlockEntity};

pub(super) struct NetherStructures {
    complexes: RandomSpreadPlacement,
    fortress_weight: u64,
    total_weight: u64,
    fossils: RandomSpreadPlacement,
    portals: RandomSpreadPlacement,
    pub(super) bastions: VillagePlanSource,
    fossil_templates: Vec<[Stamp; 4]>,
    portal_templates: Vec<[Stamp; 4]>,
    bricks: BlockStateId,
    fence: BlockStateId,
    wart: BlockStateId,
    spawner: BlockStateId,
    chest: BlockStateId,
    tags: Arc<CacheTags>,
    biome_tags: [Identifier; 3],
}

struct Stamp {
    blocks: Vec<TemplateBlock>,
    chests: Vec<TemplateChest>,
    block_entities: Vec<TemplateBlockEntity>,
    air_cells: Vec<[i32; 3]>,
    min: [i32; 3],
    max: [i32; 3],
}

fn id(name: &str) -> Identifier {
    Identifier::parse(name).expect("static identifier")
}

fn biome_tag(root: &std::path::Path, name: &str) -> Result<Identifier, String> {
    let path = root
        .join("data/minecraft/worldgen/structure")
        .join(format!("{name}.json"));
    let value: serde_json::Value = serde_json::from_slice(
        &std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?,
    )
    .map_err(|e| e.to_string())?;
    let tag = value["biomes"]
        .as_str()
        .and_then(|s| s.strip_prefix('#'))
        .ok_or_else(|| format!("{name}: expected biome tag"))?;
    Identifier::parse(tag).map_err(|e| e.to_string())
}

fn nether_portal_state(
    registry: &BlockRegistry,
    state: BlockStateId,
) -> Result<BlockStateId, String> {
    let block = registry.by_id(state).ok_or("unregistered portal state")?;
    let name = match block.block.id.path() {
        "stone" | "cobblestone" | "mossy_cobblestone" => "minecraft:blackstone",
        "stone_bricks" | "mossy_stone_bricks" => "minecraft:polished_blackstone_bricks",
        "cracked_stone_bricks" => "minecraft:cracked_polished_blackstone_bricks",
        "chiseled_stone_bricks" => "minecraft:chiseled_polished_blackstone",
        "stone_brick_stairs" | "mossy_stone_brick_stairs" => {
            "minecraft:polished_blackstone_brick_stairs"
        }
        "stone_brick_slab" | "mossy_stone_brick_slab" => "minecraft:polished_blackstone_brick_slab",
        "stone_brick_wall" | "mossy_stone_brick_wall" => "minecraft:polished_blackstone_brick_wall",
        "cobblestone_stairs" | "mossy_cobblestone_stairs" => "minecraft:blackstone_stairs",
        "cobblestone_slab" | "mossy_cobblestone_slab" => "minecraft:blackstone_slab",
        "cobblestone_wall" | "mossy_cobblestone_wall" => "minecraft:blackstone_wall",
        _ => return Ok(state),
    };
    registry
        .by_name_and_props(&id(name), &block.properties)
        .ok_or_else(|| format!("unsupported Nether portal replacement {name}"))
}

fn placement(spec: &PlacementSpec) -> RandomSpreadPlacement {
    let PlacementSpec::RandomSpread(spec) = spec;
    RandomSpreadPlacement {
        spacing: spec.spacing,
        separation: spec.separation,
        salt: i64::from(spec.salt),
        triangular: spec.spread_type == RandomSpreadType::Triangular,
    }
}

fn templates(
    root: &std::path::Path,
    names: impl Iterator<Item = String>,
    registry: &BlockRegistry,
) -> Result<Vec<[Stamp; 4]>, String> {
    names
        .map(|name| {
            let template = StructureTemplate::from_nbt_file(
                root.join("data/minecraft/structure")
                    .join(format!("{name}.nbt")),
                registry,
            )
            .map_err(|e| e.to_string())?;
            let mut rotations = Vec::with_capacity(4);
            for rotation in [
                Rotation::None,
                Rotation::Clockwise90,
                Rotation::Clockwise180,
                Rotation::CounterClockwise90,
            ] {
                let blocks = template
                    .blocks()
                    .iter()
                    .map(|block| {
                        Ok(TemplateBlock {
                            pos: rotation.transform(block.pos),
                            state: rotate_state(
                                registry,
                                if name.starts_with("ruined_portal/") {
                                    nether_portal_state(registry, block.state)?
                                } else {
                                    block.state
                                },
                                rotation,
                            )
                            .ok_or_else(|| format!("{name}: unsupported rotation"))?,
                        })
                    })
                    .collect::<Result<Vec<_>, String>>()?;
                let chests = template
                    .chests()
                    .iter()
                    .map(|chest| {
                        let mut chest = chest.clone();
                        chest.pos = rotation.transform(chest.pos);
                        chest
                    })
                    .collect();
                let block_entities = template
                    .block_entities()
                    .iter()
                    .map(|entity| {
                        let mut entity = entity.clone();
                        entity.pos = rotation.transform(entity.pos);
                        entity
                    })
                    .collect();
                let mut min = [i32::MAX; 3];
                let mut max = [i32::MIN; 3];
                let size = template.size();
                for x in [0, size[0] - 1] {
                    for y in [0, size[1] - 1] {
                        for z in [0, size[2] - 1] {
                            let p = rotation.transform([x, y, z]);
                            for axis in 0..3 {
                                min[axis] = min[axis].min(p[axis]);
                                max[axis] = max[axis].max(p[axis]);
                            }
                        }
                    }
                }
                let air_cells = template
                    .air_cells()
                    .iter()
                    .map(|p| rotation.transform(*p))
                    .collect();
                rotations.push(Stamp {
                    blocks,
                    chests,
                    block_entities,
                    air_cells,
                    min,
                    max,
                });
            }
            Ok(rotations.try_into().ok().expect("four rotations"))
        })
        .collect()
}

impl NetherStructures {
    pub fn load(
        root: &std::path::Path,
        generator: &NetherGenerator,
        tags: Arc<CacheTags>,
    ) -> Result<Self, String> {
        let loader = VillageDataLoader::new(root.join("data/minecraft/worldgen"));
        let complex_id = id("minecraft:nether_complexes");
        let mut set = loader
            .load_structure_set(&complex_id)
            .map_err(|e| e.to_string())?;
        let complexes = placement(&set.placement);
        let total_weight = set.structures.iter().map(|s| s.weight as u64).sum();
        let fortress_weight = set
            .structures
            .iter()
            .find(|s| s.structure.path() == "fortress")
            .ok_or("nether_complexes lacks fortress")?
            .weight as u64;
        set.structures
            .retain(|s| s.structure.path() == "bastion_remnant");
        if set.structures.len() != 1 || total_weight <= fortress_weight {
            return Err("nether_complexes lacks bastion".into());
        }
        let mut closure = load_jigsaw_closure(root, &complex_id, set, &generator.registry)
            .map_err(|e| e.to_string())?;
        closure.dimension_bounds = (0, 128);
        let closure = Arc::new(closure);
        let decor = Arc::new(
            VillageDecor::compile(
                root,
                &closure,
                &BlockSemantics::new(&generator.registry, tags.as_ref()),
            )
            .map_err(|e| e.to_string())?,
        );
        let bastions = VillagePlanSource::new(
            closure,
            decor,
            generator.seed,
            Arc::clone(&generator.registry),
            tags.clone(),
            tags.clone(),
        )
        .map_err(|e| e.to_string())?;
        let fossils = placement(
            &loader
                .load_structure_set(&id("minecraft:nether_fossils"))
                .map_err(|e| e.to_string())?
                .placement,
        );
        let portals = placement(
            &loader
                .load_structure_set(&id("minecraft:ruined_portals"))
                .map_err(|e| e.to_string())?
                .placement,
        );
        let b = |name| try_resolve(&generator.registry, name).map_err(|e| e.to_string());
        Ok(Self {
            complexes,
            fortress_weight,
            total_weight,
            fossils,
            portals,
            bastions,
            fossil_templates: templates(
                root,
                (1..=14).map(|n| format!("nether_fossils/fossil_{n}")),
                &generator.registry,
            )?,
            portal_templates: templates(
                root,
                (1..=10)
                    .map(|n| format!("ruined_portal/portal_{n}"))
                    .chain((1..=3).map(|n| format!("ruined_portal/giant_portal_{n}"))),
                &generator.registry,
            )?,
            bricks: b("minecraft:nether_bricks")?,
            fence: b("minecraft:nether_brick_fence")?,
            wart: b("minecraft:nether_wart")?,
            spawner: b("minecraft:spawner")?,
            chest: b("minecraft:chest")?,
            biome_tags: [
                biome_tag(root, "fortress")?,
                biome_tag(root, "nether_fossil")?,
                biome_tag(root, "ruined_portal_nether")?,
            ],
            tags,
        })
    }

    fn fortress(&self, generator: &NetherGenerator, x: i32, z: i32) -> bool {
        // A failed bastion biome gate retries the fortress from this same set,
        // instead of reducing complex density in basalt deltas.
        generator.hash(x, 0, z, 0x434f4d504c4558) % self.total_weight < self.fortress_weight
            || self
                .bastions
                .plan_for_start_chunk(x, z, &|_, _| 33, &|x, _, z| {
                    generator.biomes[generator.biome_index(x, z)].clone()
                })
                .is_none()
    }

    fn allowed(&self, generator: &NetherGenerator, kind: usize, x: i32, z: i32) -> bool {
        self.tags.biome_in_tag(
            &self.biome_tags[kind],
            &generator.biomes[generator.biome_index(x, z)],
        )
    }

    fn starts(
        &self,
        generator: &NetherGenerator,
        placement: RandomSpreadPlacement,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Vec<(i32, i32)> {
        let mut starts = Vec::new();
        if min.0 > max.0 || min.1 > max.1 {
            return starts;
        }
        for gx in min.0.div_euclid(placement.spacing)..=max.0.div_euclid(placement.spacing) {
            for gz in min.1.div_euclid(placement.spacing)..=max.1.div_euclid(placement.spacing) {
                let p = placement.potential_chunk(
                    generator.seed,
                    gx * placement.spacing,
                    gz * placement.spacing,
                );
                if (min.0..=max.0).contains(&p.0) && (min.1..=max.1).contains(&p.1) {
                    starts.push(p);
                }
            }
        }
        starts.sort_unstable();
        starts
    }

    pub fn sites(
        &self,
        generator: &NetherGenerator,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Vec<GeneratedStructureSite> {
        let mut out = Vec::new();
        for (cx, cz) in self.starts(generator, self.complexes, min, max) {
            if self.fortress(generator, cx, cz)
                && self.allowed(generator, 0, cx * 16 + 8, cz * 16 + 8)
            {
                out.push(GeneratedStructureSite {
                    structure: id("minecraft:fortress"),
                    position: BlockPos {
                        x: cx * 16 + 8,
                        y: 65,
                        z: cz * 16 + 8,
                    },
                    start_chunk: (cx, cz),
                });
            } else if let Some(plan) =
                self.bastions
                    .plan_for_start_chunk(cx, cz, &|_, _| 33, &|x, _, z| {
                        generator.biomes[generator.biome_index(x, z)].clone()
                    })
            {
                let pos = plan.assembly().origin;
                out.push(GeneratedStructureSite {
                    structure: id("minecraft:bastion_remnant"),
                    position: pos,
                    start_chunk: (cx, cz),
                });
            }
        }
        for (placement, name) in [
            (self.fossils, "minecraft:nether_fossil"),
            (self.portals, "minecraft:ruined_portal_nether"),
        ] {
            for (cx, cz) in self.starts(generator, placement, min, max) {
                let (x, z) = (cx * 16 + 8, cz * 16 + 8);
                if !self.allowed(
                    generator,
                    if name.ends_with("fossil") { 1 } else { 2 },
                    x,
                    z,
                ) {
                    continue;
                }
                if let Some(y) = generator.floor(x, z) {
                    out.push(GeneratedStructureSite {
                        structure: id(name),
                        position: BlockPos { x, y: y + 1, z },
                        start_chunk: (cx, cz),
                    });
                }
            }
        }
        out.sort_by_key(|s| (s.start_chunk, s.structure.clone()));
        out
    }

    pub fn write(&self, generator: &NetherGenerator, chunk: &mut Chunk) {
        let radius = self.bastions.neighbourhood_chunk_radius().max(3);
        let min = (chunk.pos.x - radius, chunk.pos.z - radius);
        let max = (chunk.pos.x + radius, chunk.pos.z + radius);
        let mut bastions = Vec::new();
        for (cx, cz) in self.starts(generator, self.complexes, min, max) {
            if self.fortress(generator, cx, cz)
                && self.allowed(generator, 0, cx * 16 + 8, cz * 16 + 8)
            {
                if (cx - chunk.pos.x).abs() <= 3 && (cz - chunk.pos.z).abs() <= 3 {
                    self.write_fortress(generator, chunk, cx * 16 + 8, cz * 16 + 8);
                }
                let (x, z) = (cx * 16 + 8, cz * 16 + 8);
                let site = GeneratedStructureSite {
                    structure: id("minecraft:fortress"),
                    position: BlockPos { x, y: 65, z },
                    start_chunk: (cx, cz),
                };
                mc_world::structure_starts::insert(
                    chunk,
                    &site,
                    BlockPos {
                        x: x - 32,
                        y: 30,
                        z: z - 32,
                    },
                    BlockPos {
                        x: x + 32,
                        y: 71,
                        z: z + 32,
                    },
                );
            } else if let Some(plan) =
                self.bastions
                    .plan_for_start_chunk(cx, cz, &|_, _| 33, &|x, _, z| {
                        generator.biomes[generator.biome_index(x, z)].clone()
                    })
                && plan.affects_chunk(chunk.pos.x, chunk.pos.z)
            {
                bastions.push(Arc::new(plan));
            }
        }
        if let Some(plans) = VillagePlanSet::from_plans(bastions) {
            self.bastions
                .write_plans(chunk, &plans, &mut [false; 256], &|_, _| 33, generator.air)
                .expect("validated bastion templates");
        }
        for (placement, is_fossil) in [(self.fossils, true), (self.portals, false)] {
            for (cx, cz) in self.starts(
                generator,
                placement,
                (chunk.pos.x - 3, chunk.pos.z - 3),
                (chunk.pos.x + 3, chunk.pos.z + 3),
            ) {
                let (x, z) = (cx * 16 + 8, cz * 16 + 8);
                if !self.allowed(generator, if is_fossil { 1 } else { 2 }, x, z) {
                    continue;
                }
                let Some(y) = generator.floor(x, z) else {
                    continue;
                };
                let hash = generator.hash(
                    cx,
                    0,
                    cz,
                    if is_fossil {
                        0x464f5353494c
                    } else {
                        0x504f5254414c
                    },
                );
                let templates = if is_fossil {
                    &self.fossil_templates
                } else {
                    &self.portal_templates
                };
                let index = if is_fossil {
                    hash as usize % templates.len()
                } else if hash % 100 < 5 {
                    10 + (hash >> 8) as usize % 3
                } else {
                    (hash >> 8) as usize % 10
                };
                let stamp = &templates[index][(hash >> 32) as usize % 4];
                let origin = BlockPos { x, y: y + 1, z };
                if !is_fossil && hash & 1 == 0 {
                    for p in &stamp.air_cells {
                        put(chunk, x + p[0], y + 1 + p[1], z + p[2], generator.air);
                    }
                }
                stamp.write(chunk, origin, hash);
                let world = |p: [i32; 3]| BlockPos {
                    x: x + p[0],
                    y: (y + 1 + p[1]).clamp(1, 122),
                    z: z + p[2],
                };
                let site = GeneratedStructureSite {
                    structure: id(if is_fossil {
                        "minecraft:nether_fossil"
                    } else {
                        "minecraft:ruined_portal_nether"
                    }),
                    position: origin,
                    start_chunk: (cx, cz),
                };
                mc_world::structure_starts::insert(
                    chunk,
                    &site,
                    world(stamp.min),
                    world(stamp.max),
                );
            }
        }
    }

    fn write_fortress(&self, generator: &NetherGenerator, chunk: &mut Chunk, x: i32, z: i32) {
        // Connected cross bridges with four end rooms. The layout is deliberately
        // Solaris-owned; it supplies the vanilla progression contents without
        // copying the procedural Mojang fortress piece implementation.
        for lx in 0..16 {
            for lz in 0..16 {
                let wx = chunk.pos.x * 16 + lx;
                let wz = chunk.pos.z * 16 + lz;
                let dx = wx - x;
                let dz = wz - z;
                let bridge = (dx.abs() <= 32 && dz.abs() <= 2) || (dz.abs() <= 32 && dx.abs() <= 2);
                let room = [(27, 0), (-27, 0), (0, 27), (0, -27)]
                    .into_iter()
                    .any(|(a, b)| (dx - a).abs() <= 5 && (dz - b).abs() <= 5);
                if !bridge && !room {
                    continue;
                }
                for y in 62..=64 {
                    put(chunk, wx, y, wz, self.bricks);
                }
                for y in 65..=71 {
                    put(chunk, wx, y, wz, generator.air);
                }
                if room {
                    let border =
                        [(27, 0), (-27, 0), (0, 27), (0, -27)]
                            .into_iter()
                            .any(|(a, b)| {
                                let (u, v) = ((dx - a).abs(), (dz - b).abs());
                                (u == 5 && v <= 5) || (v == 5 && u <= 5)
                            });
                    if border && !bridge {
                        for y in 65..=69 {
                            put(chunk, wx, y, wz, self.bricks);
                        }
                    }
                    put(chunk, wx, 70, wz, self.bricks);
                } else if (dz.abs() == 2 && dx.abs() > 3) || (dx.abs() == 2 && dz.abs() > 3) {
                    put(chunk, wx, 65, wz, self.fence);
                }
                if (dx.rem_euclid(8) == 0 && dz.abs() <= 1)
                    || (dz.rem_euclid(8) == 0 && dx.abs() <= 1)
                {
                    for y in 30..62 {
                        put(chunk, wx, y, wz, self.bricks);
                    }
                }
                if (dx + 27).abs() <= 3 && dz.abs() <= 3 && dz != 0 {
                    put(chunk, wx, 64, wz, generator.palette.soul_sand);
                    put(chunk, wx, 65, wz, self.wart);
                }
            }
        }
        let spawner = BlockPos {
            x: x + 27,
            y: 66,
            z,
        };
        if owns(chunk, spawner) {
            put(chunk, spawner.x, spawner.y, spawner.z, self.spawner);
            let entity = TemplateBlockEntity {
                pos: [0, 0, 0],
                state: self.spawner,
                nbt: vec![
                    ("id".into(), Tag::String("minecraft:mob_spawner".into())),
                    (
                        "SpawnData".into(),
                        Tag::Compound(vec![(
                            "entity".into(),
                            Tag::Compound(vec![(
                                "id".into(),
                                Tag::String("minecraft:blaze".into()),
                            )]),
                        )]),
                    ),
                    ("Delay".into(), Tag::Short(20)),
                    ("MinSpawnDelay".into(), Tag::Short(200)),
                    ("MaxSpawnDelay".into(), Tag::Short(800)),
                    ("SpawnCount".into(), Tag::Short(4)),
                    ("MaxNearbyEntities".into(), Tag::Short(6)),
                    ("RequiredPlayerRange".into(), Tag::Short(16)),
                    ("SpawnRange".into(), Tag::Short(4)),
                ],
            };
            chunk
                .block_entities
                .insert(spawner, entity.at_world_position(spawner));
        }
        for dz in [-27, 27] {
            let pos = BlockPos {
                x,
                y: 65,
                z: z + dz,
            };
            if owns(chunk, pos) {
                put(chunk, pos.x, pos.y, pos.z, self.chest);
                let chest = TemplateChest {
                    pos: [0, 0, 0],
                    chest: ChestBlockEntity::default(),
                    loot_table: Some(id("minecraft:chests/nether_bridge")),
                };
                chunk.chests.insert(
                    pos,
                    chest.unopened_contents(generator.hash(pos.x, pos.y, pos.z, 0x4c4f4f54)),
                );
            }
        }
    }
}

fn owns(chunk: &Chunk, pos: BlockPos) -> bool {
    pos.x.div_euclid(16) == chunk.pos.x
        && pos.z.div_euclid(16) == chunk.pos.z
        && (1..123).contains(&pos.y)
}

impl Stamp {
    fn write(&self, chunk: &mut Chunk, origin: BlockPos, seed: u64) {
        let world = |p: [i32; 3]| BlockPos {
            x: origin.x + p[0],
            y: origin.y + p[1],
            z: origin.z + p[2],
        };
        for block in &self.blocks {
            let pos = world(block.pos);
            put(chunk, pos.x, pos.y, pos.z, block.state);
        }
        for chest in &self.chests {
            let pos = world(chest.pos);
            if owns(chunk, pos) {
                chunk.chests.insert(
                    pos,
                    chest.unopened_contents(splitmix64(
                        seed ^ (pos.x as u64) ^ (pos.z as u64).rotate_left(32),
                    )),
                );
            }
        }
        for entity in &self.block_entities {
            let pos = world(entity.pos);
            if owns(chunk, pos) {
                chunk
                    .block_entities
                    .insert(pos, entity.at_world_position(pos));
            }
        }
    }
}
