//! Plugin-authored block tools: a held plugin-owned item used on a block replaces
//! it, wears the item and grants an item, all as one native intent.
//!
//! Rules are immutable startup data (`mc_data::block_tools`); this module only
//! admits them against the block registry and plans a use. The commit lives in
//! the tick with the other held-item block edits (`IntentKind::BlockTool`), so a
//! use is accepted or refused whole against the world and inventory it was
//! planned on. A rule is never consulted for a stack whose model/carrier pair is
//! not the exact admitted custom item.

use std::collections::BTreeMap;

use mc_data::block_tools::{BlockToolRule, CLICKED_FACE, COPY_PREFIX, ToolFace};
use mc_data::{Identifier, ItemStack};
use mc_world::{BlockRegistry, BlockState, BlockStateId};

use crate::boundary::{BlockPos, EntityId, Hand};
use crate::intent::{Footprint, Intent, IntentKind, InvClaim};
use crate::placement::Face;
use crate::tick::TickState;

const FACES: [Face; 6] = [
    Face::Down,
    Face::Up,
    Face::North,
    Face::South,
    Face::West,
    Face::East,
];

/// Admitted rules indexed by `(tool item, source block)`, in authoring order.
#[derive(Debug, Clone, Default)]
pub(crate) struct BlockToolTable {
    rules: Vec<BlockToolRule>,
    index: BTreeMap<(Identifier, Identifier), Vec<usize>>,
}

/// The decision for one use: the state to write, durability to charge and the
/// item to grant.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ToolPlan {
    pub result: u32,
    pub wear: i32,
    pub give: Option<Identifier>,
}

fn face_name(face: Face) -> &'static str {
    match face {
        Face::Down => "down",
        Face::Up => "up",
        Face::North => "north",
        Face::South => "south",
        Face::West => "west",
        Face::East => "east",
    }
}

fn face_matches(filter: ToolFace, face: Face) -> bool {
    match filter {
        ToolFace::Any => true,
        ToolFace::Up => face == Face::Up,
        ToolFace::Down => face == Face::Down,
        ToolFace::Side => !matches!(face, Face::Up | Face::Down),
    }
}

impl BlockToolTable {
    pub(crate) fn is_empty(&self) -> bool {
        self.rules.is_empty()
    }

    pub(crate) fn rules(&self) -> &[BlockToolRule] {
        &self.rules
    }

    /// Admit every rule against the registry: both blocks exist, `when` names real
    /// properties and values, and the replacement resolves to a registered state
    /// for every face the rule can be used on. A rule that could only fail at use
    /// time is a startup error instead.
    pub(crate) fn admit(blocks: &BlockRegistry, rules: Vec<BlockToolRule>) -> Result<Self, String> {
        let mut index: BTreeMap<(Identifier, Identifier), Vec<usize>> = BTreeMap::new();
        for (position, rule) in rules.iter().enumerate() {
            let source = blocks
                .block(&rule.block)
                .ok_or_else(|| format!("block tool source {} is not a block", rule.block))?;
            blocks
                .block(&rule.result)
                .ok_or_else(|| format!("block tool result {} is not a block", rule.result))?;
            let mut source_props = blocks
                .by_id(source.default)
                .ok_or_else(|| format!("block {} has no default state", rule.block))?
                .properties
                .clone();
            for (name, value) in &rule.when {
                source_props
                    .iter_mut()
                    .find(|(key, _)| key == name)
                    .ok_or_else(|| format!("{} has no property {name}", rule.block))?
                    .1 = value.clone();
            }
            let sample = blocks
                .by_name_and_props(&rule.block, &source_props)
                .and_then(|id| blocks.by_id(id))
                .ok_or_else(|| {
                    format!(
                        "{} has no state matching `when` {:?}",
                        rule.block, rule.when
                    )
                })?;
            for face in FACES
                .into_iter()
                .filter(|face| face_matches(rule.face, *face))
            {
                resolve(blocks, rule, sample, face).ok_or_else(|| {
                    format!(
                        "block tool {} -> {} has no registered state for face {}",
                        rule.block,
                        rule.result,
                        face_name(face)
                    )
                })?;
            }
            index
                .entry((rule.item.clone(), rule.block.clone()))
                .or_default()
                .push(position);
        }
        Ok(Self { rules, index })
    }

    /// The first rule of `item` matching the clicked state and face whose result
    /// differs from what is there.
    pub(crate) fn plan(
        &self,
        blocks: &BlockRegistry,
        item: &Identifier,
        source: u32,
        face: Face,
    ) -> Option<ToolPlan> {
        let state = blocks.by_id(BlockStateId(source))?;
        let candidates = self.index.get(&(item.clone(), state.block.id.clone()))?;
        candidates.iter().find_map(|&position| {
            let rule = &self.rules[position];
            let applies = face_matches(rule.face, face)
                && rule.when.iter().all(|(name, value)| {
                    state
                        .properties
                        .iter()
                        .any(|(key, current)| key == name && current == value)
                });
            let result =
                resolve(blocks, rule, state, face).filter(|&id| applies && id != source)?;
            Some(ToolPlan {
                result,
                wear: i32::try_from(rule.wear).unwrap_or(i32::MAX),
                give: rule.give.clone(),
            })
        })
    }
}

/// The replacement state of `rule` for a clicked state and face.
fn resolve(
    blocks: &BlockRegistry,
    rule: &BlockToolRule,
    source: &BlockState,
    face: Face,
) -> Option<u32> {
    let mut props = blocks
        .by_id(blocks.block(&rule.result)?.default)?
        .properties
        .clone();
    for (name, value) in &rule.set {
        let value = if value == CLICKED_FACE {
            face_name(face).to_owned()
        } else if let Some(copied) = value.strip_prefix(COPY_PREFIX) {
            source
                .properties
                .iter()
                .find(|(key, _)| key == copied)?
                .1
                .clone()
        } else {
            value.clone()
        };
        props.iter_mut().find(|(key, _)| key == name)?.1 = value;
    }
    blocks
        .by_name_and_props(&rule.result, &props)
        .map(|id| id.0)
}

/// A main-hand use of a plugin-owned tool on `clicked`: the intent that replaces
/// the block, charges the wear and grants the item, `None` when the held item is
/// not an admitted tool or no rule applies there.
pub(crate) fn use_intent(
    state: &mut TickState,
    entity: EntityId,
    hand: Hand,
    clicked: BlockPos,
    face: Face,
    held: &ItemStack,
) -> Option<Intent> {
    if hand != Hand::Main || state.registry.block_tools.is_empty() {
        return None;
    }
    let tool = state
        .registry
        .item_facts
        .custom_for_stack(held, &state.registry.items)?
        .id
        .clone();
    let player = state.players.get(&entity)?;
    if player.spectator() {
        return None;
    }
    let creative = player.creative();
    let slot = crate::tick::held_slot_index(player, hand);
    let current = state.world.block_at(clicked.x, clicked.y, clicked.z)?;
    let plan = state
        .registry
        .block_tools
        .plan(&state.registry.blocks, &tool, current, face)?;
    // Creative players neither wear tools nor receive conserved material.
    let give = match (&plan.give, creative) {
        (Some(item), false) => Some(state.registry.items.id_of(item)?),
        _ => None,
    };
    let intent = Intent {
        actor: entity,
        seq: state.next_seq(entity),
        kind: IntentKind::BlockTool {
            wear: if creative { 0 } else { plan.wear },
            give,
        },
        footprint: Footprint {
            block_expect: vec![(clicked, current)],
            block_write: vec![(clicked, plan.result)],
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: 0,
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    };
    Some(intent)
}
