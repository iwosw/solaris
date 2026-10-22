//! Native death-grave custody: one container at the death spot instead of loose
//! drops.
//!
//! This is the native half of the `death_graves` deployment declaration. The
//! server's own death path decides custody; this module only answers where the
//! container goes and moves the exact stacks the death produced into it, in the
//! same tick and under the same journal. Product policy - who may open it and
//! when it becomes public - belongs to the plugin that declared the capability
//! and is applied through the native zone owner.
//!
//! Every stack either lands in the container or stays in the caller's drop list,
//! which the caller spawns exactly as vanilla would. There is no second copy of a
//! stack anywhere in this module: a duplicate would need the same stack to be both
//! stored and left behind, and one `mem::take` per stack forbids that.

use mc_data::Identifier;

use crate::boundary::BlockPos;
use crate::tick::TickState;

/// The block a grave is made of.
///
/// A barrel rather than a chest: barrels never pair into a double container, so
/// placing a grave beside an existing chest cannot join two inventories.
const GRAVE_BLOCK: &str = "minecraft:barrel";

/// How far from the death cell a safe placement may look, in blocks.
const GRAVE_SEARCH_RADIUS: i32 = 2;

/// What one burial did, as the declaring package observes it.
pub(crate) struct Buried {
    pub position: BlockPos,
    pub dimension: String,
    /// Stacks the container took.
    pub items: u32,
    /// Stacks that did not fit and stay in the caller's drop list.
    pub spilled: u32,
}

/// Move a death's drops into a grave container placed near the death spot.
///
/// `drops` keeps whatever the container did not take; the caller spawns those as
/// ordinary item entities. `None` means no container was placed at all (no
/// replaceable, grounded, unprotected cell next to the death), so every stack
/// stays in the caller's list.
pub(crate) fn bury(
    state: &mut TickState,
    owner: &str,
    death: [f64; 3],
    drops: &mut Vec<mc_data::ItemStack>,
) -> Option<Buried> {
    if drops.is_empty() {
        return None;
    }
    let position = place(state, owner, death)?;
    let storage = state
        .block_entities
        .get_mut(&(position.x, position.y, position.z))?;
    let mut slot = 0;
    let mut taken = 0;
    while slot < storage.slots.len() && taken < drops.len() {
        if storage.slots[slot].is_empty() {
            storage.slots[slot] = std::mem::take(&mut drops[taken]);
            taken += 1;
        }
        slot += 1;
    }
    // The image changed after the block-entity default was written, so it must be
    // journaled as a container mutation of this tick.
    state.world.mark_block_entity_dirty(position);
    drops.drain(..taken);
    let items = u32::try_from(taken).unwrap_or(u32::MAX);
    let spilled = u32::try_from(drops.len()).unwrap_or(u32::MAX);
    Some(Buried {
        position,
        dimension: state.dimension.name().to_owned(),
        items,
        spilled,
    })
}

/// Place the grave block and create its empty container inventory.
///
/// Candidates are the death cell, the cell above it, then a square ring around
/// the death at one above, level and one below, so a grave still appears beside a
/// player who died on a pillar or in a doorway. A candidate is accepted only when
/// the world is loaded there, the block is replaceable, the cell stands on solid
/// ground that is not a hopper, and the cell does not sit inside another
/// package's protected zone.
fn place(state: &mut TickState, owner: &str, death: [f64; 3]) -> Option<BlockPos> {
    let block = state
        .registry
        .blocks
        .block(&Identifier::parse(GRAVE_BLOCK).ok()?)?;
    let written = block.default.0;
    let dimension = state.dimension.name().to_owned();
    let base = BlockPos {
        x: coordinate(death[0], mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT)?,
        y: coordinate(death[1], mc_script::SCRIPT_VERTICAL_COORDINATE_LIMIT)?,
        z: coordinate(death[2], mc_script::SCRIPT_HORIZONTAL_COORDINATE_LIMIT)?,
    };
    for position in candidates(base) {
        let Some(current) = state.world.block_at(position.x, position.y, position.z) else {
            continue;
        };
        // `replaceable` admits water and lava. A drowning death in an ocean still
        // deserves its grave, so water is accepted; lava is not: the barrel would
        // burn and the vanilla drop beside it is the honest outcome.
        if matches!(
            state.registry.materials.classify(current),
            mc_physics::BlockMaterial::Lava
        ) {
            continue;
        }
        if !crate::tick::replaceable(state, current) {
            continue;
        }
        // A grave stands on one full collision block that is not a hopper.
        //
        // The full block is what makes the private window durable against
        // machinery: the zone protects it, so nobody can break it and drop a
        // hopper or a rail there later, and a minecart cannot roll into a cell a
        // full block occupies. An existing hopper below is full-collision too but
        // it already pulls, so it is excluded explicitly.
        let below = BlockPos {
            y: position.y - 1,
            ..position
        };
        if !solid_ground(state, below) || hopper_block(state, below) {
            continue;
        }
        if !crate::plugin_player::allows_plugin_bounds(
            state,
            owner,
            &dimension,
            [position.x, position.y, position.z],
            [position.x, position.y, position.z],
        ) {
            continue;
        }
        state.stage_container_removal(position, false);
        state
            .world
            .set_block(position.x, position.y, position.z, written);
        state.plugin_block_writes.push((position, written));
        crate::block_entity_defaults::after_blocks(state, &[(position, written)]);
        // The write is already in the world: either this cell now holds the
        // container and its inventory, or the caller keeps every stack. Trying a
        // second cell would leave this barrel behind as an empty one.
        return state
            .block_entities
            .contains_key(&(position.x, position.y, position.z))
            .then_some(position);
    }
    None
}

fn coordinate(value: f64, limit: f64) -> Option<i32> {
    let floored = value.floor();
    (floored.is_finite() && floored.abs() <= limit).then_some(floored as i32)
}

/// A cell a grave may stand on: loaded ground whose collision is one full block.
///
/// The material alone is not enough: `BlockMaterial::Solid` also covers partial
/// shapes such as slabs, and a partial cell leaves room for a hopper minecart to
/// roll under the grave. The registry's resolved collision boxes are the exact
/// predicate the rest of the engine uses, and only `[FULL_BLOCK]` closes that
/// route (a slab is one partial box, a rail no box at all).
fn solid_ground(state: &TickState, below: BlockPos) -> bool {
    state
        .world
        .block_at(below.x, below.y, below.z)
        .and_then(|id| state.registry.collision_boxes.get(id as usize))
        .is_some_and(|boxes| boxes.as_ref() == [mc_physics::BlockCollisionBox::FULL_BLOCK])
}

/// Whether the cell holds a hopper, which pulls the container above it.
fn hopper_block(state: &TickState, below: BlockPos) -> bool {
    state
        .world
        .block_at(below.x, below.y, below.z)
        .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
        .is_some_and(|block| block.block.id.path() == "hopper")
}

/// Candidate cells in placement order, bounded by the search radius.
fn candidates(base: BlockPos) -> Vec<BlockPos> {
    let mut cells = Vec::with_capacity(1 + 3 * (2 * GRAVE_SEARCH_RADIUS as usize + 1).pow(2));
    cells.push(base);
    cells.push(BlockPos {
        y: base.y + 1,
        ..base
    });
    for radius in 1..=GRAVE_SEARCH_RADIUS {
        for dy in [0, 1, -1] {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx.abs() != radius && dz.abs() != radius {
                        continue;
                    }
                    cells.push(BlockPos {
                        x: base.x + dx,
                        y: base.y + dy,
                        z: base.z + dz,
                    });
                }
            }
        }
    }
    cells
}
