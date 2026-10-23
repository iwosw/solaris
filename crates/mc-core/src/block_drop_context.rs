//! Block-entity custody before block writes clear its native chunk image.
use crate::boundary::{BlockPos, ChunkPos};
use crate::tick::TickState;
use mc_nbt::{ListTag, Tag, tag_type};
use mc_world::BlockStateId;

pub(crate) fn capture(
    state: &TickState,
    pos: BlockPos,
    block: u32,
) -> Result<Option<Tag>, crate::CoreError> {
    let Some(block) = state.registry.blocks.by_id(BlockStateId(block)) else {
        return Ok(None);
    };
    if !state.registry.loot.block_needs_entity(&block.block.id) {
        return Ok(None);
    }
    if matches!(
        block.block.id.as_str(),
        "minecraft:beehive" | "minecraft:bee_nest"
    ) && let Some(image) = crate::bees::loot_image(state, pos)
    {
        return decode_image(&image).map(Some);
    }
    let raw = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|chunk| chunk.block_entities.get(&pos));
    let live = state
        .block_entities
        .get(&(pos.x, pos.y, pos.z))
        .filter(|_| {
            state
                .registry
                .loot
                .block_copies_component(&block.block.id, "minecraft:container")
        });
    let root = raw.map(|bytes| decode_image(bytes)).transpose()?;
    let Some(live) = live else {
        return Ok(root);
    };
    let mut root = root.unwrap_or_else(|| Tag::Compound(Vec::new()));
    let Tag::Compound(fields) = &mut root else {
        return Err(crate::CoreError::Storage(
            "block loot: block entity root must be a compound".into(),
        ));
    };
    let mut items = Vec::new();
    for (slot, stack) in live
        .slots
        .iter()
        .enumerate()
        .filter(|(_, stack)| !stack.is_empty())
    {
        let mut item =
            mc_data::item_nbt::stack_to_fields(&state.registry.items, stack).map_err(|error| {
                crate::CoreError::Storage(format!(
                    "block loot: invalid container item NBT: {error}"
                ))
            })?;
        let slot = u8::try_from(slot).map_err(|_| {
            crate::CoreError::Storage("block loot: container slot exceeds 255".into())
        })?;
        item.push(("Slot".into(), Tag::Byte(slot as i8)));
        items.push(Tag::Compound(item));
    }
    let items = Tag::List(ListTag {
        element_type: tag_type::COMPOUND,
        elements: items,
    });
    if let Some((_, old)) = fields.iter_mut().find(|(name, _)| name == "Items") {
        *old = items;
    } else {
        fields.push(("Items".into(), items));
    }
    Ok(Some(root))
}

fn decode_image(mut bytes: &[u8]) -> Result<Tag, crate::CoreError> {
    let root = mc_nbt::read_network(&mut bytes).map_err(|error| {
        crate::CoreError::Storage(format!("block loot: corrupt block entity NBT: {error}"))
    })?;
    if !bytes.is_empty() {
        return Err(crate::CoreError::Storage(
            "block loot: trailing block entity NBT".into(),
        ));
    }
    if !matches!(root, Tag::Compound(_)) {
        return Err(crate::CoreError::Storage(
            "block loot: block entity root must be a compound".into(),
        ));
    }
    Ok(root)
}

pub(crate) fn admit(
    state: &mut TickState,
    pos: BlockPos,
    drops: Result<Vec<mc_data::ItemStack>, crate::CoreError>,
) -> Option<Vec<mc_data::ItemStack>> {
    if state
        .world
        .load_error(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .is_some()
    {
        return None;
    }
    let result = drops.and_then(|mut drops| {
        crate::bees::break_custody(state, pos, &mut drops)?;
        Ok(drops)
    });
    match result {
        Ok(drops) => Some(drops),
        Err(error) => {
            reject(state, pos, error);
            None
        }
    }
}

pub(crate) fn reject(state: &mut TickState, pos: BlockPos, error: crate::CoreError) {
    state.world.reject_resident(
        ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        },
        error.to_string(),
    );
}
/// A reserved carrier state drops its custom item, never the carrier's loot.
fn carrier_drop(state: &TickState, block: u32) -> Option<Vec<mc_data::ItemStack>> {
    state
        .registry
        .carrier_blocks
        .drop_for_state(block, &state.registry.item_facts, &state.registry.items)
        .map(|stack| vec![stack])
}

pub(crate) fn explosion_drops(
    state: &TickState,
    pos: BlockPos,
    block: u32,
    radius: Option<f32>,
    seed: u64,
) -> Result<Vec<mc_data::ItemStack>, crate::CoreError> {
    if let Some(drops) = carrier_drop(state, block) {
        return Ok(drops);
    }
    let entity = capture(state, pos, block)?;
    crate::block_drops::explosion_drops(
        &crate::block_drops::BlockDropRules {
            blocks: &state.registry.blocks,
            loot: &state.registry.loot,
            mining: &state.registry.mining,
        },
        &state.registry.item_rules(),
        block,
        seed,
        entity.as_ref(),
        radius,
    )
}

pub(crate) fn retains_contents(drops: &[mc_data::ItemStack]) -> bool {
    drops.iter().any(|stack| {
        stack
            .extended()
            .preserved_components
            .iter()
            .any(|component| component.name == "minecraft:container")
    })
}

pub(crate) fn roll(
    state: &TickState,
    pos: BlockPos,
    block: u32,
    tool: &mc_data::ItemStack,
    seed: u64,
) -> Result<Vec<mc_data::ItemStack>, crate::CoreError> {
    if let Some(drops) = carrier_drop(state, block) {
        return Ok(drops);
    }
    let entity = capture(state, pos, block)?;
    crate::block_drops::block_drops(
        &crate::block_drops::BlockDropRules {
            blocks: &state.registry.blocks,
            loot: &state.registry.loot,
            mining: &state.registry.mining,
        },
        &state.registry.item_rules(),
        block,
        tool,
        seed,
        entity.as_ref(),
    )
}

pub(crate) fn player_drops(
    state: &TickState,
    actor: crate::boundary::EntityId,
    pos: BlockPos,
    block: u32,
) -> Result<Vec<mc_data::ItemStack>, crate::CoreError> {
    if state.connections.entity_connection(actor).is_none() {
        return Ok(Vec::new());
    }
    let Some(player) = state.players.get(&actor) else {
        return Ok(Vec::new());
    };
    if player.game_mode == crate::commands::GameMode::Creative {
        if state
            .registry
            .blocks
            .by_id(BlockStateId(block))
            .is_some_and(|block| crate::shulker::is_block(block.block.id.path()))
            && state
                .block_entities
                .get(&(pos.x, pos.y, pos.z))
                .is_some_and(|record| record.slots.iter().any(|stack| !stack.is_empty()))
        {
            // ShulkerBoxBlock.playerWillDestroy preserves nonempty creative
            // boxes by dropping one item with the collected BE components.
            return roll(state, pos, block, &mc_data::ItemStack::EMPTY, state.tick);
        }
        return Ok(Vec::new());
    }
    if !crate::command_rules::boolean(state, "block_drops") {
        return Ok(Vec::new());
    }
    let empty_tool = mc_data::ItemStack::EMPTY;
    let tool = player
        .inventory
        .slots
        .get(usize::from(player.inventory.selected.min(8)))
        .unwrap_or(&empty_tool);
    roll(
        state,
        pos,
        block,
        tool,
        state
            .tick
            .wrapping_mul(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(actor.0 as u64),
    )
}

#[cfg(test)]
#[path = "block_drop_context_tests.rs"]
mod tests;
