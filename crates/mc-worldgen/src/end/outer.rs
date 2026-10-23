//! Bounded coordinate-only outer End terrain and survival landmarks.
//! Shapes are independently authored, not copied from Mojang templates/code.
use super::*;

pub(super) struct Outer {
    plant: BlockStateId,
    pub(super) flower: BlockStateId,
    biomes: [Identifier; 4],
    pub(super) placement: crate::village::placement::RandomSpreadPlacement,
}

impl Outer {
    pub(super) fn new(registry: &BlockRegistry) -> Result<Self, EndGeneratorError> {
        Ok(Self {
            plant: registry
                .by_name_and_props(
                    &Identifier::parse("minecraft:chorus_plant").expect("static identifier"),
                    &[
                        ("down", "true"),
                        ("up", "true"),
                        ("east", "false"),
                        ("west", "false"),
                        ("north", "false"),
                        ("south", "false"),
                    ]
                    .map(|(key, value)| (key.to_owned(), value.to_owned())),
                )
                .ok_or(EndGeneratorError::MissingRequiredBlock(
                    "minecraft:chorus_plant[down=true,up=true]",
                ))?,
            flower: try_resolve(registry, "minecraft:chorus_flower")?,
            biomes: [
                "end_highlands",
                "end_midlands",
                "end_barrens",
                "small_end_islands",
            ]
            .map(|s| Identifier::parse(format!("minecraft:{s}")).expect("End biome")),
            // Local 26.1.2 worldgen/structure_set/end_cities.json defaults.
            placement: crate::village::placement::RandomSpreadPlacement {
                spacing: 20,
                separation: 11,
                salt: 10_387_313,
                triangular: true,
            },
        })
    }
}

impl EndGenerator {
    pub(super) fn fill_biomes(&self, chunk: &mut Chunk) {
        let cells: [&Identifier; 16] = std::array::from_fn(|index| {
            self.biome_at(
                chunk.pos.x * 16 + (index % 4) as i32 * 4,
                chunk.pos.z * 16 + (index / 4) as i32 * 4,
            )
        });
        if cells.iter().all(|id| *id == cells[0]) {
            chunk
                .biomes
                .fill(mc_world::chunk::BiomeSection::filled(cells[0].clone()));
            return;
        }
        let mut palette: Vec<Identifier> = Vec::with_capacity(5);
        let mut indices = mc_world::section::PackedBitArray::zeroed(3, 64);
        for (index, biome) in cells.into_iter().enumerate() {
            let id = palette
                .iter()
                .position(|id| id == biome)
                .unwrap_or_else(|| {
                    palette.push(biome.clone());
                    palette.len() - 1
                });
            for y in 0..4 {
                indices.set(y * 16 + index, id as u32);
            }
        }
        chunk
            .biomes
            .fill(mc_world::chunk::BiomeSection::from_indirect(
                palette, indices,
            ));
    }

    fn island(&self, gx: i32, gz: i32) -> (i32, i32, f64, u64) {
        let h = splitmix64(
            self.seed as u64
                ^ (gx as u64).wrapping_mul(0x9E37_79B9)
                ^ (gz as u64).wrapping_mul(0x85EB_CA77),
        );
        (
            gx * 160 + 24 + (h % 112) as i32,
            gz * 160 + 24 + ((h >> 8) % 112) as i32,
            30.0 + ((h >> 16) % 53) as f64,
            h,
        )
    }

    pub(super) fn outer_height(&self, x: i32, z: i32) -> i32 {
        if f64::from(x).mul_add(f64::from(x), f64::from(z) * f64::from(z)) < 1_000_000.0 {
            return -1;
        }
        let mut top = -1;
        for gx in x.div_euclid(160) - 1..=x.div_euclid(160) + 1 {
            for gz in z.div_euclid(160) - 1..=z.div_euclid(160) + 1 {
                let (cx, cz, radius, h) = self.island(gx, gz);
                let dx = f64::from(x - cx);
                let dz = f64::from(z - cz);
                let distance_squared = dx.mul_add(dx, dz * dz) / (radius * radius);
                if distance_squared < 1.0 {
                    let y = 47.0 + ((h >> 32) % 15) as f64 + 16.0 * (1.0 - distance_squared);
                    top = top.max(y.floor() as i32);
                }
            }
        }
        top
    }

    pub(super) fn biome_at(&self, x: i32, z: i32) -> &Identifier {
        if (x as f64).hypot(z as f64) < 1000.0 {
            return &self.biome;
        }
        let y = self.outer_height(x, z);
        &self.outer.biomes[if y >= 65 {
            0
        } else if y >= 56 {
            1
        } else if y >= 0 {
            2
        } else {
            3
        }]
    }

    // One data-driven triangular random-spread candidate per placement region.
    pub(super) fn city(&self, gx: i32, gz: i32) -> Option<(i32, i32, i32, u64)> {
        let placement = self.outer.placement;
        let (cx, cz) =
            placement.potential_chunk(self.seed, gx * placement.spacing, gz * placement.spacing);
        let (x, z) = (cx * 16 + 8, cz * 16 + 8);
        let y = [(x, z), (x + 5, z), (x, z + 5), (x + 5, z + 5)]
            .map(|(x, z)| self.outer_height(x, z))
            .into_iter()
            .min()?;
        if y < 60 || self.biome_at(x, z) != &self.outer.biomes[0] {
            return None;
        }
        let h = splitmix64(self.seed as u64 ^ (cx as u64).wrapping_mul(0x517C_C1B7) ^ cz as u64);
        Some((x, y + 1, z, h))
    }

    pub(super) fn decorate_outer(&self, chunk: &mut Chunk) {
        let bx = chunk.pos.x * 16;
        let bz = chunk.pos.z * 16;
        if ((bx as f64).hypot(bz as f64)) < 960.0 {
            return;
        }
        // Column-local stems do not depend on neighboring chunk generation order.
        for x in bx..bx + 16 {
            for z in bz..bz + 16 {
                let y = self.outer_height(x, z);
                let h =
                    splitmix64(self.seed as u64 ^ (x as u64).wrapping_mul(0x517C_C1B7) ^ z as u64);
                if y >= 60 && h % 53 == 0 {
                    let height = 2 + ((h >> 16) % 4) as i32;
                    for dy in 1..height {
                        put(chunk, x, y + dy, z, self.outer.plant);
                    }
                    put(chunk, x, y + height, z, self.outer.flower);
                }
            }
        }
        let Some(cities) = &self.cities else { return };
        let mut markers = Vec::new();
        let spacing = self.outer.placement.spacing * 16;
        for gx in (bx - 160).div_euclid(spacing)..=(bx + 160).div_euclid(spacing) {
            for gz in (bz - 160).div_euclid(spacing)..=(bz + 160).div_euclid(spacing) {
                let Some((x, y, z, h)) = self.city(gx, gz) else {
                    continue;
                };
                if x + 96 < bx || x - 96 >= bx + 16 || z + 96 < bz || z - 96 >= bz + 16 {
                    continue;
                }
                cities.paste(self, chunk, [x, y, z], h, &mut markers);
            }
        }
        if !markers.is_empty() {
            chunk.set_generated_entities(&markers);
        }
    }
}

pub(super) fn put(chunk: &mut Chunk, x: i32, y: i32, z: i32, state: BlockStateId) {
    let lx = x - chunk.pos.x * 16;
    let lz = z - chunk.pos.z * 16;
    if (0..16).contains(&lx) && (0..16).contains(&lz) {
        let _ = chunk.set_block(lx as u8, y, lz as u8, state);
    }
}
