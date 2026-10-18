//! What a broken block drops (ADR 0013): vanilla block loot tables evaluated
//! with the breaking tool, gated by "requires correct tool for drops". Pure;
//! the merge step spawns the returned stacks as item entities.

use mc_data::ItemStack;
use mc_data::block_mining::BlockMiningTable;
use mc_data::item_components::ToolFacts;
use mc_data::loot::{BlockLootContext, LootContextItem, LootRandomBinding, LootTables};
use mc_data::tags::TagsData;
use mc_world::{Block, BlockRegistry, BlockStateId};

use crate::inventory::ItemRules;

/// Immutable block data the drops read; built once at startup.
#[derive(Clone, Copy)]
pub struct BlockDropRules<'a> {
    pub blocks: &'a BlockRegistry,
    pub loot: &'a LootTables,
    pub mining: &'a BlockMiningTable,
}

/// Stacks dropped when a survival player breaks `state` with `tool`
/// (empty stack for a bare hand). `seed` drives the loot rolls.
pub fn block_drops(
    drops: &BlockDropRules<'_>,
    items: &ItemRules<'_>,
    state: u32,
    tool: &ItemStack,
    seed: u64,
    block_entity: Option<&mc_nbt::Tag>,
) -> Result<Vec<ItemStack>, crate::CoreError> {
    let Some(block_state) = drops.blocks.by_id(BlockStateId(state)) else {
        return Ok(Vec::new());
    };
    let block = &block_state.block;
    if drops
        .mining
        .requires_correct_tool_for_drops(state)
        .unwrap_or(false)
        && !correct_tool(items, block, tool)
    {
        return Ok(Vec::new());
    }
    evaluate(drops, items, state, tool, seed, block_entity, None)
}

pub(crate) fn explosion_drops(
    drops: &BlockDropRules<'_>,
    items: &ItemRules<'_>,
    state: u32,
    seed: u64,
    block_entity: Option<&mc_nbt::Tag>,
    radius: Option<f32>,
) -> Result<Vec<ItemStack>, crate::CoreError> {
    evaluate(
        drops,
        items,
        state,
        &ItemStack::EMPTY,
        seed,
        block_entity,
        radius,
    )
}

fn evaluate(
    drops: &BlockDropRules<'_>,
    items: &ItemRules<'_>,
    state: u32,
    tool: &ItemStack,
    seed: u64,
    block_entity: Option<&mc_nbt::Tag>,
    radius: Option<f32>,
) -> Result<Vec<ItemStack>, crate::CoreError> {
    let Some(block_state) = drops.blocks.by_id(BlockStateId(state)) else {
        return Ok(Vec::new());
    };
    let block = &block_state.block;
    let Some(context_tool) = loot_tool(items, tool) else {
        return Ok(Vec::new());
    };
    let random = LootRandomBinding::new(drops.loot.block_random_sequence(&block.id).cloned(), seed);
    let Ok(mut context) =
        BlockLootContext::try_new(&block.id, &block_state.properties, &context_tool, random)
    else {
        return Ok(Vec::new());
    };
    if let Some(entity) = block_entity {
        context = context.with_block_entity(entity);
    }
    if let Some(radius) = radius {
        context = context.with_explosion(
            mc_data::loot::LootExplosion::try_new(radius)
                .map_err(|error| crate::CoreError::Storage(error.to_string()))?,
        );
    }
    let rolled = match drops.loot.evaluate_block(&context) {
        Ok(Some(rolled)) => rolled,
        Err(error @ mc_data::loot::BlockLootEvaluationError::InvalidBlockEntity { .. }) => {
            return Err(crate::CoreError::Storage(error.to_string()));
        }
        _ => return Ok(Vec::new()),
    };
    let mut stacks = Vec::with_capacity(rolled.len());
    for drop in rolled {
        let Some(id) = items.items.id_of(&drop.item) else {
            continue;
        };
        let Some(count) = drop
            .count
            .try_sample(0)
            .ok()
            .and_then(|count| i32::try_from(count).ok())
        else {
            continue;
        };
        if count <= 0 {
            continue;
        }
        let stack = if let Some(components) = drop.components {
            mc_data::item_nbt::stack_from_fields(
                items.items,
                &[
                    (
                        "id".into(),
                        mc_nbt::Tag::String(drop.item.as_str().to_owned()),
                    ),
                    ("count".into(), mc_nbt::Tag::Int(count)),
                    ("components".into(), *components),
                ],
            )
            .map_err(|error| {
                crate::CoreError::Storage(format!(
                    "block loot: invalid copied item components: {error}"
                ))
            })?
            .ok_or_else(|| {
                crate::CoreError::Storage("block loot: copied item is missing its id".into())
            })?
        } else {
            ItemStack::new(id, count)
        };
        stacks.push(stack);
    }
    Ok(stacks)
}

/// Vanilla block-break experience, suppressed by silk touch and tool gating.
pub(crate) fn block_experience(
    drops: &BlockDropRules<'_>,
    items: &ItemRules<'_>,
    state: u32,
    tool: &ItemStack,
    seed: u64,
) -> i32 {
    let Some(block_state) = drops.blocks.by_id(BlockStateId(state)) else {
        return 0;
    };
    if tool.enchantments.iter().any(|enchantment| {
        enchantment.id.as_str() == "minecraft:silk_touch" && enchantment.level > 0
    }) || (drops
        .mining
        .requires_correct_tool_for_drops(state)
        .unwrap_or(false)
        && !correct_tool(items, &block_state.block, tool))
    {
        return 0;
    }
    let (min, max) = match block_state.block.id.as_str() {
        "minecraft:coal_ore" | "minecraft:deepslate_coal_ore" => (0, 2),
        "minecraft:diamond_ore"
        | "minecraft:deepslate_diamond_ore"
        | "minecraft:emerald_ore"
        | "minecraft:deepslate_emerald_ore" => (3, 7),
        "minecraft:lapis_ore" | "minecraft:deepslate_lapis_ore" | "minecraft:nether_quartz_ore" => {
            (2, 5)
        }
        "minecraft:redstone_ore" | "minecraft:deepslate_redstone_ore" => (1, 5),
        "minecraft:sculk" => (1, 1),
        "minecraft:sculk_catalyst"
        | "minecraft:sculk_sensor"
        | "minecraft:calibrated_sculk_sensor"
        | "minecraft:sculk_shrieker" => (5, 5),
        _ => return 0,
    };
    let roll = seed.wrapping_mul(0xbf58_476d_1ce4_e5b9);
    min + ((roll ^ (roll >> 32)) % (max - min + 1) as u64) as i32
}

/// The tool as loot context: item id and its enchantments (silk touch,
/// fortune). Plugin items are not their carrier and count as a bare hand.
fn loot_tool(items: &ItemRules<'_>, tool: &ItemStack) -> Option<LootContextItem> {
    if tool.is_empty()
        || tool
            .item_model
            .as_deref()
            .is_some_and(|model| items.facts.custom(model).is_some())
    {
        return Some(LootContextItem::empty());
    }
    let name = items.items.name_of(tool.item_id)?;
    let mut context = LootContextItem::new(name.clone());
    for enchantment in &tool.enchantments {
        context = context
            .try_with_enchantment(
                enchantment.id.clone(),
                u32::try_from(enchantment.level.max(0)).unwrap_or(0),
            )
            .ok()?;
    }
    Some(context)
}

/// Vanilla `Tool.isCorrectForDrops`: the first rule that matches the block
/// and states `correct_for_drops` decides.
pub(crate) fn correct_tool(items: &ItemRules<'_>, block: &Block, tool: &ItemStack) -> bool {
    let Some(facts) = (!tool.is_empty())
        .then(|| items.facts.facts_for_stack(tool, items.items))
        .flatten()
        .and_then(|facts| facts.tool.as_ref())
    else {
        return false;
    };
    first_drop_rule(facts, block, items.tags) == Some(true)
}

fn first_drop_rule(tool: &ToolFacts, block: &Block, tags: &TagsData) -> Option<bool> {
    tool.rules.iter().find_map(|rule| {
        rule.correct_for_drops.filter(|_| {
            rule.blocks.iter().any(|target| {
                if target.starts_with('#') {
                    tags.contains_raw_id("minecraft:block", target, block.raw_id)
                } else {
                    target == block.id.as_str()
                }
            })
        })
    })
}

#[cfg(test)]
#[path = "block_drops_tests.rs"]
mod tests;
