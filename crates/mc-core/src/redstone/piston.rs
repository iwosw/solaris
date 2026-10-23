//! Bounded, deterministic piston move planning.
//!
//! A move is planned as one atomic group: either every edit of the push (or
//! pull) is planned, or nothing is. The planner refuses rather than partially
//! applying, which is what keeps a move from duplicating or destroying blocks
//! and block-entity contents.

use mc_data::Identifier;
use mc_world::{BlockPos, BlockRegistry, BlockState, BlockStateId};

use super::blocks::{NEIGHBOURS, facing_delta, offset};
use super::planning::Settle;
use super::planning::{BlockPlanningRead, block_state_property};

/// Vanilla push limit, shared by the entire attached structure.
pub(super) const MAX_PUSH_DISTANCE: usize = 12;

/// Why a piston could not move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Refusal {
    /// The state does not describe a piston this slice can move.
    Unsupported,
    /// A destination is occupied by an immovable block or block entity.
    Immovable(BlockPos),
    /// More than [`MAX_PUSH_DISTANCE`] blocks would move.
    PushLimit,
    /// The transaction touches a protected position.
    Protected(BlockPos),
    /// A position in the path is not resident, so the move cannot be validated.
    Unloaded(BlockPos),
    /// The registry has no `piston_head` state for the required facing/type.
    MissingHead,
}

/// Plan the extend/retract group and its entity sweep. `None` means no move.
pub(super) fn plan_move<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    state: &BlockState,
    sticky: bool,
    powered: bool,
) -> Result<Option<super::piston_motion::Motion>, Refusal> {
    let facing = block_state_property(state, "facing").ok_or(Refusal::Unsupported)?;
    let extended = block_state_property(state, "extended") == Some("true");
    if powered == extended {
        return Ok(None);
    }
    let step = facing_delta(facing).ok_or(Refusal::Unsupported)?;
    let head = offset(pos, step).ok_or(Refusal::Unloaded(pos))?;
    let base_state =
        super::planning::sibling_state_with_bool_property(world.blocks, state, "extended", powered)
            .ok_or(Refusal::Unsupported)?;

    if powered {
        plan_extend(world, pos, head, step, base_state, facing, sticky)
    } else {
        plan_retract(world, pos, head, step, base_state, sticky)
    }
}

/// Retire source claims against the whole preimage before publishing the move.
pub(super) fn apply_edits(
    state: &mut crate::tick::TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<crate::boundary::EntityId>,
    edits: Vec<super::planning::BlockEdit>,
    motion: Option<super::piston_motion::Motion>,
) {
    // Roll every destroyed block against the preimage before committing any
    // part of the structure. Moving blocks themselves never produce loot.
    let mut drops = Vec::new();
    if crate::command_rules::boolean(state, "block_drops") {
        for edit in &edits {
            let old = state
                .world
                .block_at(edit.pos.x, edit.pos.y, edit.pos.z)
                .expect("validated piston preimage remains resident during atomic apply");
            if state
                .registry
                .blocks
                .by_id(BlockStateId(old))
                .is_some_and(|block| destroys_on_push(block.block.id.path()))
            {
                match crate::block_drop_context::roll(
                    state,
                    edit.pos,
                    old,
                    &mc_data::ItemStack::EMPTY,
                    state.tick,
                ) {
                    Ok(stacks) => {
                        drops.extend(stacks.into_iter().map(|stack| (edit.pos, stack)));
                    }
                    Err(error) => {
                        crate::block_drop_context::reject(state, edit.pos, error);
                        return;
                    }
                }
            }
        }
    }
    if let Some(motion) = motion {
        super::piston_motion::apply(state, motion);
    }
    for edit in &edits {
        crate::plugin_inventories::invalidate_block(state, edit.pos);
    }
    for edit in edits {
        super::set(state, writes, edit.pos, edit.new_state.0);
    }
    for (pos, stack) in drops {
        spawned.push(crate::tick::spawn_item_entity(
            state,
            [
                f64::from(pos.x) + 0.5,
                f64::from(pos.y) + 0.5,
                f64::from(pos.z) + 0.5,
            ],
            stack,
        ));
    }
}

fn plan_extend<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    head: BlockPos,
    step: (i32, i32, i32),
    base_state: BlockStateId,
    facing: &str,
    sticky: bool,
) -> Result<Option<super::piston_motion::Motion>, Refusal> {
    let mut structure = resolve(world, pos, None, head, step, true)?;
    validate(world, pos)?;
    validate(world, head)?;
    let head_state = head_state(world.blocks, facing, sticky).ok_or(Refusal::MissingHead)?;
    publish(world, &structure, step);
    world.plan(pos, base_state);
    world.plan(head, head_state);
    structure.moved.push((pos, head_state));
    Ok(Some(super::piston_motion::Motion {
        step,
        blocks: structure.moved,
    }))
}

fn plan_retract<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    pos: BlockPos,
    head: BlockPos,
    step: (i32, i32, i32),
    base_state: BlockStateId,
    sticky: bool,
) -> Result<Option<super::piston_motion::Motion>, Refusal> {
    let air = super::planning::air_state_id(world.blocks);
    // Only a real piston head is cleared/pulled: a base flagged extended whose
    // head block is gone must not delete whatever now occupies that position.
    let head_state = world.state_at(head).ok_or(Refusal::Unloaded(head))?;
    let head_id = head_state.id;
    let head_is_head = head_state.block.id.path() == "piston_head"
        && block_state_property(head_state, "facing").and_then(facing_delta) == Some(step)
        && block_state_property(head_state, "type")
            == Some(if sticky { "sticky" } else { "normal" });
    // A failed pull leaves the structure in place, but still retracts the
    // piston. An unloaded footprint must wait, not silently abandon the pull.
    let structure = if sticky && head_is_head {
        let behind = offset(head, step).ok_or(Refusal::Unloaded(head))?;
        match resolve(
            world,
            pos,
            Some(head),
            behind,
            (-step.0, -step.1, -step.2),
            false,
        ) {
            Ok(structure) => Some(structure),
            Err(error @ (Refusal::Unloaded(_) | Refusal::Protected(_))) => return Err(error),
            Err(_) => None,
        }
    } else {
        None
    };
    validate(world, pos)?;
    if head_is_head {
        validate(world, head)?;
    }
    world.plan(pos, base_state);
    if head_is_head {
        world.plan(head, air);
    }
    let mut blocks = if let Some(structure) = structure {
        publish(world, &structure, (-step.0, -step.1, -step.2));
        structure.moved
    } else {
        Vec::new()
    };
    if head_is_head {
        blocks.push((head, head_id));
    }
    Ok(Some(super::piston_motion::Motion {
        step: (-step.0, -step.1, -step.2),
        blocks,
    }))
}

#[derive(Default)]
struct Structure {
    moved: Vec<(BlockPos, BlockStateId)>,
    destroyed: Vec<BlockPos>,
}

fn validate<R: BlockPlanningRead + ?Sized>(
    world: &Settle<'_, R>,
    pos: BlockPos,
) -> Result<(), Refusal> {
    if world.protection_allows(pos) {
        Ok(())
    } else {
        Err(Refusal::Protected(pos))
    }
}

/// Resolve the dependency closure before editing: every moving block needs its
/// destination, and sticky blocks additionally drag movable neighbours. The
/// source list is also the bounded work queue, so cycles are visited once.
fn resolve<R: BlockPlanningRead + ?Sized>(
    world: &Settle<'_, R>,
    base: BlockPos,
    cleared_head: Option<BlockPos>,
    start: BlockPos,
    step: (i32, i32, i32),
    extending: bool,
) -> Result<Structure, Refusal> {
    let mut structure = Structure::default();
    let initial = world.state_at(start).ok_or(Refusal::Unloaded(start))?;
    if super::blocks::is_air(initial) {
        return Ok(structure);
    }
    // A sticky piston cannot pull a fragile or push-only block.
    if !extending
        && (destroys_on_push(initial.block.id.path())
            || initial.block.id.path().ends_with("_glazed_terracotta"))
    {
        return Ok(structure);
    }
    include(world, &mut structure, start)?;
    let mut index = 0;
    while index < structure.moved.len() {
        let (source, state_id) = structure.moved[index];
        let destination = offset(source, step).ok_or(Refusal::Unloaded(source))?;
        if destination == base {
            return Err(Refusal::Immovable(base));
        }
        if Some(destination) != cleared_head {
            include(world, &mut structure, destination)?;
        }
        validate(world, source)?;
        validate(world, destination)?;
        let state = world
            .blocks
            .by_id(state_id)
            .ok_or(Refusal::Unloaded(source))?;
        if sticky_block(state.block.id.path()) {
            for direction in NEIGHBOURS {
                // Forward collisions are mandatory even for blocks that do
                // not adhere (notably honey against slime and glazed blocks).
                if direction == step {
                    continue;
                }
                let neighbour = offset(source, direction).ok_or(Refusal::Unloaded(source))?;
                if neighbour == base || Some(neighbour) == cleared_head {
                    continue;
                }
                let other = world
                    .state_at(neighbour)
                    .ok_or(Refusal::Unloaded(neighbour))?;
                let other_path = other.block.id.path();
                if !super::blocks::is_air(other)
                    && adheres(state.block.id.path(), other_path)
                    && movable(other)
                    && !destroys_on_push(other_path)
                {
                    include(world, &mut structure, neighbour)?;
                }
            }
        }
        index += 1;
    }
    Ok(structure)
}

fn include<R: BlockPlanningRead + ?Sized>(
    world: &Settle<'_, R>,
    structure: &mut Structure,
    pos: BlockPos,
) -> Result<(), Refusal> {
    if structure.moved.iter().any(|(source, _)| *source == pos)
        || structure.destroyed.contains(&pos)
    {
        return Ok(());
    }
    let state_id = world.block_at(pos).ok_or(Refusal::Unloaded(pos))?;
    let state = world.blocks.by_id(state_id).ok_or(Refusal::Unloaded(pos))?;
    if super::blocks::is_air(state) {
        return Ok(());
    }
    if destroys_on_push(state.block.id.path()) {
        validate(world, pos)?;
        structure.destroyed.push(pos);
    } else {
        if !movable(state) {
            return Err(Refusal::Immovable(pos));
        }
        if structure.moved.len() == MAX_PUSH_DISTANCE {
            return Err(Refusal::PushLimit);
        }
        structure.moved.push((pos, state_id));
    }
    Ok(())
}

fn publish<R: BlockPlanningRead + ?Sized>(
    world: &mut Settle<'_, R>,
    structure: &Structure,
    step: (i32, i32, i32),
) {
    let air = super::planning::air_state_id(world.blocks);
    // Clear the preimage first, then fill destinations from the retained
    // states. Branch collisions and loops cannot erase another moved block.
    for &(pos, _) in &structure.moved {
        world.plan(pos, air);
    }
    for &pos in &structure.destroyed {
        world.plan(pos, air);
    }
    for &(pos, state) in &structure.moved {
        world.plan(
            offset(pos, step).expect("validated piston destination"),
            state,
        );
    }
}

fn sticky_block(path: &str) -> bool {
    matches!(path, "slime_block" | "honey_block")
}

fn adheres(a: &str, b: &str) -> bool {
    !b.ends_with("_glazed_terracotta")
        && !matches!(
            (a, b),
            ("slime_block", "honey_block") | ("honey_block", "slime_block")
        )
}

fn movable(state: &BlockState) -> bool {
    match state.block.id.path() {
        "piston" | "sticky_piston" => block_state_property(state, "extended") != Some("true"),
        path => !is_immovable_path(path),
    }
}

/// Vanilla DESTROY reactions do not join adhesive branches, but pop when a
/// moving block actually enters their cell.
/// Derived from every 26.1.2 block state's cached push reaction; the oracle
/// fixture tests the compressed families against the full registry. Inventory
/// and opaque block entities retain this core's conservative refusal policy.
fn destroys_on_push(path: &str) -> bool {
    if path != "comparator"
        && mc_data::block_entity_kinds::BlockEntityKind::for_block(path).is_some()
    {
        return false;
    }
    path.ends_with("_sapling")
        || path.starts_with("potted_")
        || path.ends_with("_flower")
        || path.ends_with("_tulip")
        || path.ends_with("_leaves")
        || path.ends_with("_door")
        || path.ends_with("_button")
        || path.ends_with("_pressure_plate")
        || path.ends_with("_torch")
        || (!path.starts_with("dead_")
            && (path.ends_with("_coral")
                || path.ends_with("_coral_fan")
                || path.ends_with("_coral_wall_fan")))
        || path.ends_with("_candle")
        || path.ends_with("_candle_cake")
        || path.ends_with("copper_lantern")
        || matches!(
            path,
            "redstone_wire"
                | "lever"
                | "repeater"
                | "comparator"
                | "torch"
                | "snow"
                | "fire"
                | "soul_fire"
                | "water"
                | "lava"
                | "short_grass"
                | "tall_grass"
                | "fern"
                | "large_fern"
                | "dead_bush"
                | "bush"
                | "firefly_bush"
                | "seagrass"
                | "tall_seagrass"
                | "dandelion"
                | "poppy"
                | "blue_orchid"
                | "allium"
                | "azure_bluet"
                | "oxeye_daisy"
                | "cornflower"
                | "lily_of_the_valley"
                | "wither_rose"
                | "sunflower"
                | "lilac"
                | "rose_bush"
                | "peony"
                | "pink_petals"
                | "wildflowers"
                | "leaf_litter"
                | "cactus_flower"
                | "spore_blossom"
                | "brown_mushroom"
                | "red_mushroom"
                | "crimson_fungus"
                | "warped_fungus"
                | "crimson_roots"
                | "warped_roots"
                | "nether_sprouts"
                | "hanging_roots"
                | "wheat"
                | "carrots"
                | "potatoes"
                | "beetroots"
                | "nether_wart"
                | "melon_stem"
                | "attached_melon_stem"
                | "pumpkin_stem"
                | "attached_pumpkin_stem"
                | "sweet_berry_bush"
                | "cocoa"
                | "sugar_cane"
                | "bamboo"
                | "bamboo_sapling"
                | "vine"
                | "weeping_vines"
                | "weeping_vines_plant"
                | "twisting_vines"
                | "twisting_vines_plant"
                | "cave_vines"
                | "cave_vines_plant"
                | "kelp"
                | "kelp_plant"
                | "lily_pad"
                | "tripwire"
                | "tripwire_hook"
                | "ladder"
                | "candle"
                | "lantern"
                | "soul_lantern"
                | "moss_carpet"
                | "pale_moss_carpet"
                | "moss_block"
                | "pale_moss_block"
                | "pale_hanging_moss"
                | "cactus"
                | "cake"
                | "cobweb"
                | "chorus_plant"
                | "pumpkin"
                | "carved_pumpkin"
                | "jack_o_lantern"
                | "melon"
                | "sea_pickle"
                | "turtle_egg"
                | "frogspawn"
                | "glow_lichen"
                | "sculk_vein"
                | "resin_clump"
                | "pointed_dripstone"
                | "amethyst_cluster"
                | "small_amethyst_bud"
                | "medium_amethyst_bud"
                | "large_amethyst_bud"
                | "torchflower_crop"
                | "pitcher_crop"
                | "pitcher_plant"
                | "scaffolding"
                | "dragon_egg"
                | "structure_void"
                | "mangrove_propagule"
                | "short_dry_grass"
                | "tall_dry_grass"
                | "golden_dandelion"
                | "torchflower"
                | "flower_pot"
                | "bubble_column"
                | "candle_cake"
                | "budding_amethyst"
                | "azalea"
                | "flowering_azalea"
                | "big_dripleaf"
                | "big_dripleaf_stem"
                | "small_dripleaf"
                | "open_eyeblossom"
                | "closed_eyeblossom"
        )
}

/// Vanilla `piston_head` state for the facing and piston type.
fn head_state(blocks: &BlockRegistry, facing: &str, sticky: bool) -> Option<BlockStateId> {
    let head = Identifier::parse("minecraft:piston_head").ok()?;
    let kind = if sticky { "sticky" } else { "normal" };
    let full = [
        ("facing".to_string(), facing.to_string()),
        ("short".to_string(), "false".to_string()),
        ("type".to_string(), kind.to_string()),
    ];
    if let Some(state) = blocks.by_name_and_props(&head, &full) {
        return Some(state);
    }
    let without_short = [
        ("facing".to_string(), facing.to_string()),
        ("type".to_string(), kind.to_string()),
    ];
    if let Some(state) = blocks.by_name_and_props(&head, &without_short) {
        return Some(state);
    }
    blocks.by_name_and_props(&head, &[("facing".to_string(), facing.to_string())])
}

fn is_immovable_path(path: &str) -> bool {
    // Use the disk/wire identity table so newly supported inventories and
    // opaque block entities cannot silently lose their contents when dragged.
    mc_data::block_entity_kinds::BlockEntityKind::for_block(path).is_some()
        || path.ends_with("_anvil")
        || matches!(
            path,
            "piston_head"
                | "bedrock"
                | "obsidian"
                | "crying_obsidian"
                | "reinforced_deepslate"
                | "end_portal_frame"
                | "respawn_anchor"
                | "barrier"
                | "light"
                | "nether_portal"
                | "grindstone"
                | "lodestone"
                | "anvil"
        )
}
