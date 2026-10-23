use super::*;
use crate::vanilla_features::{LegacyRandom, RandomSource};
use crate::village::set_large_feature_with_salt;
use mc_data::worldgen_ores::{HeightAnchor, OrePlacementCount};

pub(super) struct OrePass {
    state: BlockStateId,
    count: u32,
    min: i32,
    max: i32,
    triangular: bool,
    size: u32,
    buried: bool,
    deltas: Option<bool>,
}

impl NetherGenerator {
    pub(super) fn load_ores(&mut self, root: &std::path::Path) -> Result<(), String> {
        let features =
            mc_data::worldgen_ores::load_ore_features(root.join("data/minecraft/worldgen"))
                .map_err(|e| e.to_string())?;
        let mut passes = Vec::new();
        for name in [
            "ore_quartz_nether",
            "ore_quartz_deltas",
            "ore_gold_nether",
            "ore_gold_deltas",
            "ore_ancient_debris_large",
            "ore_debris_small",
        ] {
            let feature = features
                .iter()
                .find(|f| f.placed_feature.path() == name)
                .ok_or_else(|| format!("missing Nether placed feature {name}"))?;
            let height = feature
                .placement
                .height
                .as_ref()
                .ok_or_else(|| format!("{name}: missing height"))?;
            let anchor = |a| match a {
                HeightAnchor::Absolute(y) | HeightAnchor::AboveBottom(y) => y,
                HeightAnchor::BelowTop(y) => 127 - y,
            };
            let count = match feature.placement.count {
                Some(OrePlacementCount::Constant(n)) => n,
                None => 1,
                _ => return Err(format!("{name}: unsupported nonconstant count")),
            };
            let target = feature
                .targets
                .first()
                .ok_or_else(|| format!("{name}: missing ore target"))?;
            let state = self
                .registry
                .block(&target.state)
                .ok_or_else(|| format!("missing {}", target.state))?
                .default;
            let min = anchor(height.min);
            let max = anchor(height.max);
            if count > 64
                || feature.size > 16
                || min < 1
                || max > 126
                || min > max
                || !matches!(height.kind.path(), "uniform" | "trapezoid")
                || !matches!(feature.discard_chance_on_air_exposure, 0.0 | 1.0)
            {
                return Err(format!("{name}: unsupported Nether ore placement"));
            }
            passes.push(OrePass {
                state,
                count,
                min,
                max,
                triangular: height.kind.path() == "trapezoid",
                size: feature.size,
                buried: feature.discard_chance_on_air_exposure == 1.0,
                deltas: if name.ends_with("_deltas") {
                    Some(true)
                } else if name.ends_with("_nether") {
                    Some(false)
                } else {
                    None
                },
            });
        }
        self.ores = passes;
        Ok(())
    }

    pub(super) fn apply_ores(&self, chunk: &mut Chunk) {
        for (index, ore) in self.ores.iter().enumerate() {
            for cx in chunk.pos.x - 1..=chunk.pos.x + 1 {
                for cz in chunk.pos.z - 1..=chunk.pos.z + 1 {
                    let mut random = LegacyRandom::new(set_large_feature_with_salt(
                        self.seed,
                        cx,
                        cz,
                        0x4f5245 + index as i64,
                    ));
                    for _ in 0..ore.count {
                        let mut x = cx * 16 + random.next_int_bounded(16);
                        let mut z = cz * 16 + random.next_int_bounded(16);
                        let range = ore.max - ore.min;
                        let mut y = ore.min
                            + if ore.triangular {
                                random.next_int_bounded(range / 2 + 1)
                                    + random.next_int_bounded((range + 1) / 2 + 1)
                            } else {
                                random.next_int_bounded(range + 1)
                            };
                        let biome_matches = ore
                            .deltas
                            .is_none_or(|d| (self.biome_index(x, z) == 4) == d);
                        for _ in 0..ore.size {
                            if biome_matches
                                && x.div_euclid(16) == chunk.pos.x
                                && z.div_euclid(16) == chunk.pos.z
                                && (1..123).contains(&y)
                            {
                                let lx = x.rem_euclid(16) as u8;
                                let lz = z.rem_euclid(16) as u8;
                                let block = chunk.get_block(lx, y, lz);
                                let substrate = block == Some(self.netherrack)
                                    || (ore.buried
                                        && (block == Some(self.palette.basalt)
                                            || block == Some(self.palette.blackstone)));
                                let enclosed = !ore.buried
                                    || (substrate
                                        && [
                                            (1, 0, 0),
                                            (-1, 0, 0),
                                            (0, 1, 0),
                                            (0, -1, 0),
                                            (0, 0, 1),
                                            (0, 0, -1),
                                        ]
                                        .into_iter()
                                        .all(
                                            |(dx, dy, dz)| {
                                                // Consult the immutable terrain, including across chunk boundaries.
                                                // Lava also rejects debris: neither exposed air nor fluid is a host.
                                                self.solid_at(x + dx, y + dy, z + dz)
                                            },
                                        ));
                                if substrate && enclosed {
                                    let _ = chunk.set_block(lx, y, lz, ore.state);
                                }
                            }
                            match random.next_int_bounded(6) {
                                0 => x += 1,
                                1 => x -= 1,
                                2 => y += 1,
                                3 => y -= 1,
                                4 => z += 1,
                                _ => z -= 1,
                            }
                        }
                    }
                }
            }
        }
    }
}
