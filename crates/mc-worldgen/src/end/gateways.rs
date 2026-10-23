//! Rare outer-highlands return gateways; configuration is imported feature data.
use super::*;
use mc_nbt::Tag;
use mc_world::BlockPos;
use std::path::Path;

pub(super) struct Gateways {
    block: BlockStateId,
    chance: u64,
    min_offset: i32,
    max_offset: i32,
    exit: [i32; 3],
    exact: bool,
}
impl Gateways {
    pub(super) fn load(root: &Path, registry: &BlockRegistry) -> Result<Self, EndGeneratorError> {
        let read = |path: &str| -> Result<serde_json::Value, EndGeneratorError> {
            let bytes = std::fs::read(root.join(path))
                .map_err(|e| EndGeneratorError::Resources(e.to_string()))?;
            serde_json::from_slice(&bytes).map_err(|e| EndGeneratorError::Resources(e.to_string()))
        };
        let placed = read("data/minecraft/worldgen/placed_feature/end_gateway_return.json")?;
        let configured =
            read("data/minecraft/worldgen/configured_feature/end_gateway_return.json")?;
        let fail = || EndGeneratorError::Resources("unsupported End return gateway feature".into());
        let modifiers = placed["placement"].as_array().ok_or_else(fail)?;
        let rarity = modifiers
            .iter()
            .find(|m| m["type"] == "minecraft:rarity_filter")
            .ok_or_else(fail)?;
        let offset = modifiers
            .iter()
            .find(|m| m["type"] == "minecraft:random_offset")
            .ok_or_else(fail)?;
        let chance = rarity["chance"]
            .as_u64()
            .filter(|n| *n > 0)
            .ok_or_else(fail)?;
        let min_offset = offset["y_spread"]["min_inclusive"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(fail)?;
        let max_offset = offset["y_spread"]["max_inclusive"]
            .as_i64()
            .and_then(|n| i32::try_from(n).ok())
            .ok_or_else(fail)?;
        if min_offset < 3 || max_offset < min_offset || max_offset > 32 {
            return Err(fail());
        }
        let exit: Vec<i32> = configured["config"]["exit"]
            .as_array()
            .ok_or_else(fail)?
            .iter()
            .map(|v| {
                v.as_i64()
                    .and_then(|n| i32::try_from(n).ok())
                    .ok_or_else(fail)
            })
            .collect::<Result<_, _>>()?;
        let exit: [i32; 3] = exit.try_into().map_err(|_| fail())?;
        let exact = configured["config"]["exact"].as_bool().ok_or_else(fail)?;
        Ok(Self {
            block: try_resolve(registry, "minecraft:end_gateway")?,
            chance,
            min_offset,
            max_offset,
            exit,
            exact,
        })
    }
}
impl EndGenerator {
    pub(super) fn decorate_gateways(&self, chunk: &mut Chunk) {
        let Some(gateway) = &self.gateways else {
            return;
        };
        // Adjacent candidates may overlap a border; evaluate each coordinate once
        // per receiving chunk, independent of generation order.
        for cx in chunk.pos.x - 1..=chunk.pos.x + 1 {
            for cz in chunk.pos.z - 1..=chunk.pos.z + 1 {
                let h = splitmix64(
                    self.seed as u64
                        ^ (cx as u64).wrapping_mul(0xF13C_7AF1)
                        ^ (cz as u64).wrapping_mul(0x5B79_2A91),
                );
                if !h.is_multiple_of(gateway.chance) {
                    continue;
                }
                let x = cx * 16 + ((h >> 20) % 16) as i32;
                let z = cz * 16 + ((h >> 28) % 16) as i32;
                if self.biome_at(x, z).path() != "end_highlands" {
                    continue;
                }
                let y = self.outer_height(x, z)
                    + 1
                    + gateway.min_offset
                    + ((h >> 36) % (gateway.max_offset - gateway.min_offset + 1) as u64) as i32;
                for dx in -1_i32..=1 {
                    for dz in -1_i32..=1 {
                        for dy in -2_i32..=2 {
                            let block = if dx == 0 && dy == 0 && dz == 0 {
                                gateway.block
                            } else if dy.abs() == 2 && dx == 0 && dz == 0
                                || dy.abs() == 1 && (dx == 0 || dz == 0)
                            {
                                self.bedrock
                            } else {
                                self.air
                            };
                            super::outer::put(chunk, x + dx, y + dy, z + dz, block);
                        }
                    }
                }
                if x.div_euclid(16) != chunk.pos.x || z.div_euclid(16) != chunk.pos.z {
                    continue;
                }
                let nbt = Tag::Compound(vec![
                    ("id".into(), Tag::String("minecraft:end_gateway".into())),
                    ("x".into(), Tag::Int(x)),
                    ("y".into(), Tag::Int(y)),
                    ("z".into(), Tag::Int(z)),
                    ("Age".into(), Tag::Long(0)),
                    ("ExactTeleport".into(), Tag::Byte(i8::from(gateway.exact))),
                    ("exit_portal".into(), Tag::IntArray(gateway.exit.to_vec())),
                ]);
                let mut bytes = Vec::new();
                mc_nbt::write_network(&mut bytes, &nbt).expect("generated gateway NBT");
                chunk.block_entities.insert(BlockPos { x, y, z }, bytes);
            }
        }
    }
}
