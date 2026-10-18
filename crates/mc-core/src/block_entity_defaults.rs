//! One block-state-driven creation boundary for commands and normal placement.
use crate::boundary::{BlockPos, ChunkPos};
use crate::tick::{BlockEntityState, TickState};
use mc_data::ItemStack;
use mc_data::block_entity_kinds::BlockEntityKind;
use mc_nbt::Tag;

/// Accepted placement creates a new owner even if break/re-place folds to the
/// same final block state. Property-only writes never enter this boundary.
pub(crate) fn accepted_placement(state: &mut TickState, pos: BlockPos, written: u32) {
    if state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(written))
        .and_then(|block| BlockEntityKind::for_block(block.block.id.path()))
        .is_some()
    {
        state.stage_container_removal(pos, false);
    }
}

pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, written) in writes {
        if state.world.block_at(pos.x, pos.y, pos.z) != Some(written) {
            continue;
        }
        let Some(kind) = state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(written))
            .and_then(|block| BlockEntityKind::for_block(block.block.id.path()))
        else {
            continue;
        };
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if state
            .world
            .chunk(chunk)
            .is_none_or(|chunk| chunk.block_entities.contains_key(&pos))
        {
            continue;
        }
        if let Some(size) = kind.inventory_size() {
            state
                .block_entities
                .entry((pos.x, pos.y, pos.z))
                .or_insert_with(|| BlockEntityState {
                    slots: vec![ItemStack::EMPTY; size],
                    furnace: matches!(
                        kind,
                        BlockEntityKind::Furnace
                            | BlockEntityKind::Smoker
                            | BlockEntityKind::BlastFurnace
                    )
                    .then(Default::default),
                });
        }
        let image = Tag::Compound(vec![
            ("id".into(), Tag::String(kind.vanilla_id().into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
        ]);
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &image).expect("native block entity defaults encode");
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
        state.opaque_changes.push((pos, Some(bytes)));
    }
}

#[cfg(test)]
#[path = "block_entity_defaults_tests.rs"]
mod tests;
