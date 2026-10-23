use super::*;
use crate::noise::{lerp, value_noise_2d, value_noise_3d};
use mc_world::{BiomeSection, PackedBitArray};

pub(super) const BIOMES: [&str; 5] = [
    "minecraft:nether_wastes",
    "minecraft:crimson_forest",
    "minecraft:warped_forest",
    "minecraft:soul_sand_valley",
    "minecraft:basalt_deltas",
];

pub(super) struct Palette {
    pub soul_sand: BlockStateId,
    pub soul_soil: BlockStateId,
    pub basalt: BlockStateId,
    pub blackstone: BlockStateId,
    pub magma: BlockStateId,
    pub nylium: [BlockStateId; 2],
    pub stems: [BlockStateId; 2],
    pub wart: [BlockStateId; 2],
    pub fungi: [BlockStateId; 2],
    pub roots: [BlockStateId; 2],
    pub shroomlight: BlockStateId,
    pub glowstone: BlockStateId,
    pub sprouts: BlockStateId,
    pub vines: [BlockStateId; 2],
}

impl Palette {
    pub fn load(registry: &BlockRegistry) -> Result<Self, NetherGeneratorError> {
        let b = |name| try_resolve(registry, name);
        Ok(Self {
            soul_sand: b("minecraft:soul_sand")?,
            soul_soil: b("minecraft:soul_soil")?,
            basalt: b("minecraft:basalt")?,
            blackstone: b("minecraft:blackstone")?,
            magma: b("minecraft:magma_block")?,
            nylium: [
                b("minecraft:crimson_nylium")?,
                b("minecraft:warped_nylium")?,
            ],
            stems: [b("minecraft:crimson_stem")?, b("minecraft:warped_stem")?],
            wart: [
                b("minecraft:nether_wart_block")?,
                b("minecraft:warped_wart_block")?,
            ],
            fungi: [
                b("minecraft:crimson_fungus")?,
                b("minecraft:warped_fungus")?,
            ],
            roots: [b("minecraft:crimson_roots")?, b("minecraft:warped_roots")?],
            shroomlight: b("minecraft:shroomlight")?,
            glowstone: b("minecraft:glowstone")?,
            sprouts: b("minecraft:nether_sprouts")?,
            vines: [
                b("minecraft:weeping_vines")?,
                b("minecraft:twisting_vines")?,
            ],
        })
    }
}

impl NetherGenerator {
    pub(super) fn hash(&self, x: i32, y: i32, z: i32, salt: u64) -> u64 {
        splitmix64(
            (self.seed as u64)
                ^ (x as u64).wrapping_mul(0x9e3779b97f4a7c15)
                ^ (z as u64).wrapping_mul(0xbf58476d1ce4e5b9)
                ^ (y as u64).wrapping_mul(0x94d049bb133111eb)
                ^ salt,
        )
    }

    pub(super) fn biome_index(&self, x: i32, z: i32) -> usize {
        let t = value_noise_2d(x as f64 / 180.0, z as f64 / 180.0, self.seed ^ 0x54454d50);
        let h = value_noise_2d(x as f64 / 180.0, z as f64 / 180.0, self.seed ^ 0x48554d49);
        // Five climate centres, with wastes around the neutral centre. These are
        // Solaris sampling scales, not a claim of bit-identical vanilla climate.
        let centres = [
            (0.0, 0.0),
            (0.65, 0.1),
            (-0.65, 0.1),
            (0.0, -0.65),
            (0.0, 0.65),
        ];
        centres
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                ((t - a.0).powi(2) + (h - a.1).powi(2))
                    .total_cmp(&((t - b.0).powi(2) + (h - b.1).powi(2)))
            })
            .expect("five climate centres")
            .0
    }

    fn density_corner(&self, x: i32, y: i32, z: i32) -> f64 {
        let broad = value_noise_3d(x as f64 / 54.0, y as f64 / 23.0, z as f64 / 54.0, self.seed);
        let detail = value_noise_3d(
            x as f64 / 23.0,
            y as f64 / 13.0,
            z as f64 / 23.0,
            self.seed ^ 0x43415645,
        );
        let boundary = ((12 - y).max(0) as f64 / 10.0) + ((y - 108).max(0) as f64 / 12.0);
        broad + detail * 0.32 + boundary - 0.12
    }

    pub(super) fn density_column(&self, x: i32, z: i32) -> [f64; 17] {
        let gx = x.div_euclid(4) * 4;
        let gz = z.div_euclid(4) * 4;
        let tx = x.rem_euclid(4) as f64 / 4.0;
        let tz = z.rem_euclid(4) as f64 / 4.0;
        std::array::from_fn(|i| {
            let y = i as i32 * 8;
            lerp(
                lerp(
                    self.density_corner(gx, y, gz),
                    self.density_corner(gx + 4, y, gz),
                    tx,
                ),
                lerp(
                    self.density_corner(gx, y, gz + 4),
                    self.density_corner(gx + 4, y, gz + 4),
                    tx,
                ),
                tz,
            )
        })
    }

    pub(super) fn solid_at(&self, x: i32, y: i32, z: i32) -> bool {
        if !(1..123).contains(&y) {
            return true;
        }
        let gx = x.div_euclid(4) * 4;
        let gy = y.div_euclid(8) * 8;
        let gz = z.div_euclid(4) * 4;
        let tx = x.rem_euclid(4) as f64 / 4.0;
        let tz = z.rem_euclid(4) as f64 / 4.0;
        let sample = |y| {
            lerp(
                lerp(
                    self.density_corner(gx, y, gz),
                    self.density_corner(gx + 4, y, gz),
                    tx,
                ),
                lerp(
                    self.density_corner(gx, y, gz + 4),
                    self.density_corner(gx + 4, y, gz + 4),
                    tx,
                ),
                tz,
            )
        };
        lerp(sample(gy), sample(gy + 8), y.rem_euclid(8) as f64 / 8.0) > 0.0
    }

    pub(super) fn solid(column: &[f64; 17], y: i32) -> bool {
        if !(1..123).contains(&y) {
            return true;
        }
        let i = y as usize / 8;
        lerp(column[i], column[i + 1], (y % 8) as f64 / 8.0) > 0.0
    }

    pub(super) fn floor(&self, x: i32, z: i32) -> Option<i32> {
        let column = self.density_column(x, z);
        (33..90)
            .rev()
            .find(|&y| Self::solid(&column, y) && !(1..=10).any(|dy| Self::solid(&column, y + dy)))
    }

    pub(super) fn fill_landscape(&self, chunk: &mut Chunk) {
        // 425 corner samples per chunk, rather than noise evaluation per block.
        let mut lattice = [[[0.0; 17]; 5]; 5];
        let ox = chunk.pos.x * 16;
        let oz = chunk.pos.z * 16;
        for (x, plane) in lattice.iter_mut().enumerate() {
            for (z, column) in plane.iter_mut().enumerate() {
                for (y, value) in column.iter_mut().enumerate() {
                    *value =
                        self.density_corner(ox + x as i32 * 4, y as i32 * 8, oz + z as i32 * 4);
                }
            }
        }
        for x in 0..16u8 {
            for z in 0..16u8 {
                let wx = ox + i32::from(x);
                let wz = oz + i32::from(z);
                let gx = usize::from(x / 4);
                let gz = usize::from(z / 4);
                let tx = f64::from(x % 4) / 4.0;
                let tz = f64::from(z % 4) / 4.0;
                let column: [f64; 17] = std::array::from_fn(|y| {
                    lerp(
                        lerp(lattice[gx][gz][y], lattice[gx + 1][gz][y], tx),
                        lerp(lattice[gx][gz + 1][y], lattice[gx + 1][gz + 1][y], tx),
                        tz,
                    )
                });
                let biome = self.biome_index(wx, wz);
                for y in 0..=NETHER_CEILING_Y {
                    let state = if y == 0 || (y >= 123 && self.ceiling_is_bedrock(wx, y, wz)) {
                        self.bedrock
                    } else if Self::solid(&column, y) {
                        if y < 123 && y > 31 && !Self::solid(&column, y + 1) {
                            match biome {
                                1 | 2 => self.palette.nylium[biome - 1],
                                3 if self.hash(wx, y, wz, 1) & 1 == 0 => self.palette.soul_sand,
                                3 => self.palette.soul_soil,
                                4 => self.palette.basalt,
                                _ => self.netherrack,
                            }
                        } else if biome == 4 && y < 123 && !Self::solid(&column, (y + 3).min(127)) {
                            self.palette.blackstone
                        } else {
                            self.netherrack
                        }
                    } else if y < NETHER_LAVA_LEVEL {
                        self.lava
                    } else {
                        self.air
                    };
                    if state != self.air {
                        let _ = chunk.set_block(x, y, z, state);
                    }
                }
            }
        }
        // Quart biome cells and locate queries share the exact climate source.
        let mut indices = PackedBitArray::zeroed(3, 64);
        for y in 0..4 {
            for z in 0..4 {
                for x in 0..4 {
                    indices.set(
                        (y * 4 + z) * 4 + x,
                        self.biome_index(ox + x as i32 * 4 + 2, oz + z as i32 * 4 + 2) as u32,
                    );
                }
            }
        }
        for section in &mut chunk.biomes {
            *section = BiomeSection::from_indirect(self.biomes.to_vec(), indices.clone());
        }
    }

    pub(super) fn decorate(&self, chunk: &mut Chunk) {
        let ox = chunk.pos.x * 16;
        let oz = chunk.pos.z * 16;
        // Rooted plants and basalt spikes on every exposed cave floor, not just
        // a top-down heightmap (the bedrock roof is not a vegetation surface).
        for x in 0..16u8 {
            for z in 0..16u8 {
                let biome = self.biome_index(ox + i32::from(x), oz + i32::from(z));
                for y in 33..117 {
                    if chunk.get_block(x, y + 1, z) != Some(self.air) {
                        continue;
                    }
                    let ground = chunk.get_block(x, y, z);
                    let hash = self.hash(ox + i32::from(x), y, oz + i32::from(z), 0x504c414e54);
                    if (1..=2).contains(&biome) && ground == Some(self.palette.nylium[biome - 1]) {
                        let state = match hash % 13 {
                            0 => self.palette.fungi[biome - 1],
                            1..=4 => self.palette.roots[biome - 1],
                            5 if biome == 2 => self.palette.sprouts,
                            6 if biome == 2 => self.palette.vines[1],
                            _ => continue,
                        };
                        let _ = chunk.set_block(x, y + 1, z, state);
                    } else if biome == 4
                        && ground == Some(self.palette.basalt)
                        && hash.is_multiple_of(5)
                    {
                        for dy in 1..=2 + (hash % 6) as i32 {
                            if chunk.get_block(x, y + dy, z) != Some(self.air) {
                                break;
                            }
                            let _ = chunk.set_block(x, y + dy, z, self.palette.basalt);
                        }
                    } else if biome == 4
                        && y < 38
                        && ground == Some(self.palette.basalt)
                        && hash.is_multiple_of(3)
                    {
                        let _ = chunk.set_block(x, y, z, self.palette.magma);
                    }
                }
            }
        }
        // Halo anchors reproduce overlapping caps in the adjacent chunk without
        // reading or mutating a neighbour: generation order cannot truncate fungi.
        for gx in (ox - 8).div_euclid(8)..=(ox + 23).div_euclid(8) {
            for gz in (oz - 8).div_euclid(8)..=(oz + 23).div_euclid(8) {
                let hash = self.hash(gx, 0, gz, 0x46554e475553);
                let x = gx * 8 + (hash % 5) as i32;
                let z = gz * 8 + ((hash >> 8) % 5) as i32;
                let biome = self.biome_index(x, z);
                if (1..=2).contains(&biome)
                    && hash & 1 == 0
                    && let Some(y) = self.floor(x, z)
                {
                    let kind = biome - 1;
                    let height = 5 + ((hash >> 16) % 4) as i32;
                    for dy in 1..=height {
                        put(chunk, x, y + dy, z, self.palette.stems[kind]);
                    }
                    for dx in -2i32..=2 {
                        for dz in -2i32..=2 {
                            for dy in height - 2..=height + 1 {
                                if dx.abs() + dz.abs() > 3 || (dx == 0 && dz == 0 && dy <= height) {
                                    continue;
                                }
                                let block = if dy == height - 2 && (dx.abs() + dz.abs()) == 2 {
                                    self.palette.shroomlight
                                } else {
                                    self.palette.wart[kind]
                                };
                                put(chunk, x + dx, y + dy, z + dz, block);
                            }
                        }
                    }
                    if kind == 0 {
                        put(chunk, x + 2, y + height - 3, z, self.palette.vines[0]);
                    }
                }
                if hash.is_multiple_of(3) {
                    let column = self.density_column(x, z);
                    if let Some(y) = (45..120).find(|&y| {
                        Self::solid(&column, y) && !(1..=5).any(|dy| Self::solid(&column, y - dy))
                    }) {
                        for dy in 1..=5 {
                            let radius = if dy < 4 { 1 } else { 0 };
                            for dx in -radius..=radius {
                                for dz in -radius..=radius {
                                    put(chunk, x + dx, y - dy, z + dz, self.palette.glowstone);
                                }
                            }
                        }
                    }
                }
            }
        }
    }
}

pub(super) fn put(chunk: &mut Chunk, x: i32, y: i32, z: i32, block: BlockStateId) {
    if x.div_euclid(16) == chunk.pos.x && z.div_euclid(16) == chunk.pos.z && (1..123).contains(&y) {
        let _ = chunk.set_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8, block);
    }
}
