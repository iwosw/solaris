//! Double chests, vanilla `ChestBlock`: a chest placed beside a single chest
//! of the same kind and facing joins it (`type` left/right on both), the
//! pair opens one 54-slot menu, and breaking one half leaves the other a
//! single chest again. Pure: the caller reads the world through closures.

use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId};

use crate::boundary::BlockPos;
use crate::placement::{Click, Face};

/// Vanilla `ChestType`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChestType {
    Single,
    Left,
    Right,
}

impl ChestType {
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Single => "single",
            Self::Left => "left",
            Self::Right => "right",
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Single => Self::Single,
            Self::Left => Self::Right,
            Self::Right => Self::Left,
        }
    }
}

/// A chest-family state (`facing` + `type`): block, facing and type.
fn parts(blocks: &BlockRegistry, state: u32) -> Option<(&Identifier, Face, ChestType)> {
    let state = blocks.by_id(BlockStateId(state))?;
    let prop = |name: &str| {
        state
            .properties
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    };
    let kind = match prop("type")? {
        "single" => ChestType::Single,
        "left" => ChestType::Left,
        "right" => ChestType::Right,
        _ => return None,
    };
    let facing = Face::from_name(prop("facing")?).filter(|face| face.horizontal())?;
    Some((&state.block.id, facing, kind))
}

/// Vanilla `getConnectedDirection`: where the other half sits.
fn connected(facing: Face, kind: ChestType) -> Face {
    match kind {
        ChestType::Left => facing.clockwise(),
        _ => facing.counter_clockwise(),
    }
}

fn step(pos: BlockPos, toward: Face) -> BlockPos {
    let [dx, dy, dz] = toward.offset();
    BlockPos {
        x: pos.x + dx,
        y: pos.y + dy,
        z: pos.z + dz,
    }
}

/// `state` with its `type` replaced.
fn retyped(blocks: &BlockRegistry, state: u32, kind: ChestType) -> Option<u32> {
    let state = blocks.by_id(BlockStateId(state))?;
    let mut props = state.properties.clone();
    props.iter_mut().find(|(key, _)| key == "type")?.1 = kind.name().to_owned();
    blocks
        .by_name_and_props(&state.block.id, &props)
        .map(|state| state.0)
}

/// What placing a chest makes: its facing and type, plus the neighbour it
/// joins with that neighbour's new state.
pub(crate) struct Placed {
    pub facing: Face,
    pub kind: ChestType,
    pub partner: Option<(BlockPos, u32)>,
}

/// Vanilla `ChestBlock.getStateForPlacement` for the chest `block` going to
/// `target`, facing `default_facing` unless it joins a neighbour.
pub(crate) fn placement(
    blocks: &BlockRegistry,
    block: &Identifier,
    target: BlockPos,
    default_facing: Face,
    click: &Click,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Placed {
    // The facing of the single chest of this kind on `toward`, if any.
    let candidate = |toward: Face| {
        let state = block_at(step(target, toward))?;
        let (id, facing, kind) = parts(blocks, state)?;
        (crate::copper::compatible_chests(id, block) && kind == ChestType::Single).then_some(facing)
    };
    let mut facing = default_facing;
    let mut kind = ChestType::Single;
    if click.sneaking && click.face.horizontal() {
        let against = click.face.opposite();
        if let Some(neighbour) = candidate(against)
            && neighbour != against
            && neighbour != click.face
        {
            facing = neighbour;
            kind = if facing.counter_clockwise() == against {
                ChestType::Right
            } else {
                ChestType::Left
            };
        }
    }
    if kind == ChestType::Single && !click.sneaking {
        if candidate(facing.clockwise()) == Some(facing) {
            kind = ChestType::Left;
        } else if candidate(facing.counter_clockwise()) == Some(facing) {
            kind = ChestType::Right;
        }
    }
    let partner = (kind != ChestType::Single)
        .then(|| {
            let pos = step(target, connected(facing, kind));
            let state = retyped(blocks, block_at(pos)?, kind.opposite())?;
            Some((pos, state))
        })
        .flatten();
    Placed {
        facing,
        kind,
        partner,
    }
}

/// Storage cells of the chest at `pos`, in menu order: a double chest's right
/// half first (vanilla `DoubleBlockCombiner`), then its left half. A single
/// chest, a half whose partner is gone, and every other block is just `pos`.
pub(crate) fn storage_cells(
    blocks: &BlockRegistry,
    pos: BlockPos,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Vec<BlockPos> {
    match storage_partner(blocks, pos, block_at) {
        Some((ChestType::Right, other)) => vec![pos, other],
        Some((_, other)) => vec![other, pos],
        None => vec![pos],
    }
}

/// Resolve paired custody without allocating a menu's slot-cell vector.
pub(crate) fn storage_partner(
    blocks: &BlockRegistry,
    pos: BlockPos,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<(ChestType, BlockPos)> {
    let (id, facing, kind) = block_at(pos).and_then(|state| parts(blocks, state))?;
    if kind == ChestType::Single {
        return None;
    }
    let other = step(pos, connected(facing, kind));
    let paired = block_at(other)
        .and_then(|state| parts(blocks, state))
        .is_some_and(|(other_id, other_facing, other_kind)| {
            crate::copper::compatible_chests(other_id, id)
                && other_facing == facing
                && other_kind == kind.opposite()
        });
    paired.then_some((kind, other))
}

/// Breaking the chest at `pos` (state `state`): the partner cell and its
/// reverted single state, when `pos` is half of a double chest.
pub(crate) fn break_partner(
    blocks: &BlockRegistry,
    pos: BlockPos,
    state: u32,
    block_at: &dyn Fn(BlockPos) -> Option<u32>,
) -> Option<(BlockPos, u32, u32)> {
    parts(blocks, state)?;
    let other = *storage_cells(blocks, pos, block_at)
        .iter()
        .find(|cell| **cell != pos)?;
    let other_state = block_at(other)?;
    Some((
        other,
        other_state,
        retyped(blocks, other_state, ChestType::Single)?,
    ))
}

#[cfg(test)]
#[path = "chests_tests.rs"]
mod tests;
