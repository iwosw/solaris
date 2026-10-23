//! Surface-bound sculk charge growth from authored feature budgets.
//! Wiki-level generation, not a bit-exact simulation of vanilla charge cursors.
use mc_data::{Identifier, vanilla_feature_closure::SculkPatchSpec};
use mc_world::{BlockPos, BlockStateId};

use super::{
    BlockSemantics, CompileError, FeatureLevel, RandomSource, placement::CompiledIntProvider,
};

#[derive(Debug, Clone)]
pub(super) struct CompiledSculkPatch {
    spec: SculkPatchSpec,
    extra: CompiledIntProvider,
    replaceable: Identifier,
    sculk: BlockStateId,
    vein: [BlockStateId; 2],
    catalyst: BlockStateId,
    sensor: [BlockStateId; 2],
    shrieker: [BlockStateId; 2],
}

impl CompiledSculkPatch {
    pub(super) fn compile(
        semantics: &BlockSemantics<'_>,
        owner: &Identifier,
        spec: &SculkPatchSpec,
    ) -> Result<Self, CompileError> {
        let state = |name: &str, overrides: &[(&str, &str)]| {
            let block = Identifier::parse(name).expect("static block identifier");
            let error = || CompileError::UnknownBlockState {
                owner: owner.clone(),
                block: block.clone(),
            };
            let schema = semantics.blocks().block(&block).ok_or_else(error)?;
            let mut properties = semantics
                .blocks()
                .by_id(schema.default)
                .ok_or_else(error)?
                .properties
                .clone();
            for (key, value) in overrides {
                let Some((_, current)) = properties.iter_mut().find(|(name, _)| name == key) else {
                    return Err(error());
                };
                *current = (*value).into();
            }
            semantics
                .blocks()
                .by_name_and_props(&block, &properties)
                .ok_or_else(error)
        };
        Ok(Self {
            spec: spec.clone(),
            extra: CompiledIntProvider::compile(owner, &spec.extra_rare_growths)?,
            replaceable: Identifier::parse("minecraft:sculk_replaceable_world_gen")
                .expect("static tag"),
            sculk: state("minecraft:sculk", &[])?,
            vein: [
                state(
                    "minecraft:sculk_vein",
                    &[("down", "true"), ("waterlogged", "false")],
                )?,
                state(
                    "minecraft:sculk_vein",
                    &[("down", "true"), ("waterlogged", "true")],
                )?,
            ],
            catalyst: state("minecraft:sculk_catalyst", &[])?,
            sensor: [
                state("minecraft:sculk_sensor", &[("waterlogged", "false")])?,
                state("minecraft:sculk_sensor", &[("waterlogged", "true")])?,
            ],
            shrieker: [
                state(
                    "minecraft:sculk_shrieker",
                    &[("can_summon", "true"), ("waterlogged", "false")],
                )?,
                state(
                    "minecraft:sculk_shrieker",
                    &[("can_summon", "true"), ("waterlogged", "true")],
                )?,
            ],
        })
    }

    fn open(
        &self,
        level: &dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        pos: BlockPos,
    ) -> bool {
        if pos.y <= level.min_y() || pos.y >= level.max_y() {
            return false;
        }
        let state = level.block_state(pos);
        semantics.is_air(state) || level.is_water_source_at(pos) || self.vein.contains(&state)
    }

    fn surface(
        &self,
        level: &dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        pos: BlockPos,
    ) -> Option<BlockPos> {
        for dy in [0, 1, -1] {
            let pos = BlockPos {
                y: pos.y + dy,
                ..pos
            };
            if !self.open(level, semantics, pos) {
                continue;
            }
            let below = level.block_state(BlockPos {
                y: pos.y - 1,
                ..pos
            });
            if below == self.sculk || semantics.in_tag(below, &self.replaceable) {
                return Some(pos);
            }
        }
        None
    }

    pub(super) fn place(
        &self,
        level: &mut dyn FeatureLevel,
        semantics: &BlockSemantics<'_>,
        random: &mut impl RandomSource,
        origin: BlockPos,
    ) -> bool {
        let Some(start) = self.surface(level, semantics, origin) else {
            return false;
        };
        let mut placed = false;
        for round in 0..self.spec.growth_rounds + self.spec.spread_rounds {
            for _ in 0..self.spec.charge_count {
                let mut cursor = start;
                let mut charge = self.spec.amount_per_charge;
                for _ in 0..self.spec.spread_attempts {
                    if charge <= 0 {
                        break;
                    }
                    let below = BlockPos {
                        y: cursor.y - 1,
                        ..cursor
                    };
                    if semantics.in_tag(level.block_state(below), &self.replaceable) {
                        level.set_block(below, self.sculk);
                        let wet = usize::from(level.is_water_source_at(cursor));
                        level.set_block(cursor, self.vein[wet]);
                        charge -= 1;
                        placed = true;
                        if round < self.spec.growth_rounds
                            && charge >= 10
                            && random.next_int_bounded(10) == 0
                        {
                            let growth = if random.next_int_bounded(11) == 0 {
                                self.shrieker[wet]
                            } else {
                                self.sensor[wet]
                            };
                            level.set_block(cursor, growth);
                            charge -= 10;
                        }
                    }
                    let (dx, dz) =
                        [(1, 0), (-1, 0), (0, 1), (0, -1)][random.next_int_bounded(4) as usize];
                    let next = BlockPos {
                        x: cursor.x + dx,
                        z: cursor.z + dz,
                        ..cursor
                    };
                    if let Some(next) = self.surface(level, semantics, next) {
                        cursor = next;
                    }
                }
            }
        }
        if self.open(level, semantics, start)
            && level.block_state(BlockPos {
                y: start.y - 1,
                ..start
            }) == self.sculk
            && random.next_float() < self.spec.catalyst_chance
        {
            level.set_block(start, self.catalyst);
            placed = true;
        }
        for _ in 0..self.extra.sample(random) {
            for _ in 0..self.spec.spread_attempts {
                let candidate = BlockPos {
                    x: start.x + random.next_int_bounded(11) - 5,
                    z: start.z + random.next_int_bounded(11) - 5,
                    ..start
                };
                let Some(pos) = self.surface(level, semantics, candidate) else {
                    continue;
                };
                if level.block_state(BlockPos {
                    y: pos.y - 1,
                    ..pos
                }) != self.sculk
                {
                    continue;
                }
                let wet = usize::from(level.is_water_source_at(pos));
                level.set_block(pos, self.shrieker[wet]);
                placed = true;
                break;
            }
        }
        placed
    }
}

#[cfg(test)]
#[path = "sculk_tests.rs"]
mod tests;
