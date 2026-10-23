//! Item-specific dispenser plans; inventory debit stays with the container owner.
use crate::boundary::BlockPos;
use crate::tick::TickState;
use mc_data::ItemStack;

pub(super) fn shulker(
    state: &mut TickState,
    source: BlockPos,
    target: BlockPos,
    held: &ItemStack,
    writes: &mut Vec<(BlockPos, u32)>,
) -> bool {
    let Some(current) = super::at(state, target) else {
        return false;
    };
    if !crate::tick::replaceable(state, current.id.0)
        || crate::plugin_player::protects_cell(state, target)
        || crate::tick::obstructed(state, target)
    {
        return false;
    }
    let Some(item) = state.registry.items.name_of(held.item_id) else {
        return false;
    };
    let Some(block) = state.registry.blocks.block(item) else {
        return false;
    };
    let Some(default) = state.registry.blocks.by_id(block.default) else {
        return false;
    };
    let Some(source_block) = super::at(state, source) else {
        return false;
    };
    let facing = super::offset(target, (0, -1, 0))
        .and_then(|below| super::at(state, below))
        .filter(|below| !super::blocks::is_air(below))
        .map_or_else(
            || super::prop(source_block, "facing").unwrap_or("north"),
            |_| "up",
        );
    let Some(new) = super::sibling(&state.registry.blocks, default, "facing", facing) else {
        return false;
    };
    let placement = match crate::shulker::placement_from_stack(state, target, Some(held)) {
        Ok(placement) => placement,
        Err(error) => {
            tracing::error!(%error, "refusing invalid dispensed shulker contents");
            return false;
        }
    };
    super::set(state, writes, target, new.0);
    crate::shulker::commit_placement(state, placement);
    true
}

pub(super) struct Bucket {
    pub(super) new: Option<u32>,
    pub(super) remainder: &'static str,
    pub(super) washed: Option<u32>,
}

/// A successful bucket action may evaporate without writing a fluid block.
pub(super) fn bucket(state: &TickState, target: BlockPos, name: &str) -> Option<Bucket> {
    let current = state.world.block_at(target.x, target.y, target.z)?;
    match name {
        "water_bucket" | "lava_bucket" => {
            let water = name == "water_bucket";
            let fluid = if water {
                crate::fluids::Fluid::Water
            } else {
                crate::fluids::Fluid::Lava
            };
            let wet = water
                .then(|| crate::fluids::waterlogged(state, current, true))
                .flatten();
            // Direct bucket placement, unlike spreading flow, may replace
            // another liquid or succeed against an existing source.
            let washed = if wet.is_some()
                || crate::fluids::fluid_at(&state.registry.states, current).is_some()
            {
                None
            } else {
                match crate::fluids::flow_into(state, target, fluid)? {
                    crate::fluids::Flow::Open => None,
                    crate::fluids::Flow::Wash(block) => Some(block),
                }
            };
            let evaporate = water && state.dimension.water_evaporates();
            let new = (!evaporate).then_some(wet.unwrap_or(if water {
                state.registry.states.water_levels[0]
            } else {
                state.registry.states.lava_levels[0]
            }));
            Some(Bucket {
                new,
                remainder: "bucket",
                washed: washed.filter(|_| !evaporate),
            })
        }
        "bucket" => {
            let (fluid, level) = crate::fluids::contained(state, current)?;
            if level != 0 {
                return None;
            }
            let new = crate::fluids::waterlogged(state, current, false)
                .unwrap_or(state.registry.states.air);
            Some(Bucket {
                new: Some(new),
                remainder: if fluid == crate::fluids::Fluid::Water {
                    "water_bucket"
                } else {
                    "lava_bucket"
                },
                washed: None,
            })
        }
        _ => None,
    }
}
