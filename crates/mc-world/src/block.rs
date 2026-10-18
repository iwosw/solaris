//! Block types and the global block-state registry.
//!
//! Built from a parsed `blocks.json` report (`mc_data::blocks`). The
//! registry is frozen after construction; vanilla's protocol assigns
//! a global numeric id per block state and we hold the same mapping
//! verbatim — we do not invent or renumber.

use std::collections::HashMap;
use std::sync::Arc;

use mc_data::Identifier;
use mc_data::blocks::{BlockReport, BlockStateReport};
use thiserror::Error;

/// Global numeric block-state id. `u32` in memory; the wire codec
/// converts to/from `VarInt` at the boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct BlockStateId(pub u32);

/// A registered block kind.
///
/// Property schema (`properties`) is the report's ordered map of
/// `name → allowed values`; we preserve that order so concrete states
/// always render properties in the same sequence (matters for keying
/// the lookup map).
#[derive(Debug)]
pub struct Block {
    pub id: Identifier,
    pub raw_id: i32,
    pub properties: Vec<(String, Vec<String>)>,
    pub default: BlockStateId,
    pub states: Vec<BlockStateId>,
}

/// One concrete block state.
#[derive(Debug)]
pub struct BlockState {
    pub block: Arc<Block>,
    pub id: BlockStateId,
    /// Property values, in the same order as `block.properties`.
    pub properties: Vec<(String, String)>,
}

impl BlockState {
    /// Whether vanilla exposes a non-empty fluid state for this block state.
    #[must_use]
    pub fn has_fluid(&self) -> bool {
        if matches!(
            self.block.id.path(),
            "water"
                | "lava"
                | "bubble_column"
                | "kelp"
                | "kelp_plant"
                | "seagrass"
                | "tall_seagrass"
        ) {
            return true;
        }

        self.properties
            .iter()
            .any(|(name, value)| name == "waterlogged" && value == "true")
    }
}

#[derive(Debug, Error)]
pub enum RegistryError {
    #[error("duplicate block id {0}")]
    DuplicateBlock(Identifier),
    #[error("duplicate block-state id {0:?}")]
    DuplicateState(BlockStateId),
    #[error("block {0} has no default state")]
    NoDefault(Identifier),
    #[error("block-state ids are not dense; first gap at id {0}")]
    NotDense(u32),
}

pub struct BlockRegistry {
    by_id: Vec<Arc<BlockState>>,
    by_name: HashMap<Identifier, Arc<Block>>,
    by_key: HashMap<(Identifier, Vec<(String, String)>), BlockStateId>,
}

impl BlockRegistry {
    /// Build the registry from a parsed `blocks.json` report. Verifies
    /// that state ids are dense from 0..n with no duplicates and that
    /// every block declares exactly one default state.
    pub fn from_report(report: &[BlockReport]) -> Result<Self, RegistryError> {
        let total_states: usize = report.iter().map(|b| b.states.len()).sum();
        let mut blocks_by_first_state: Vec<_> = report
            .iter()
            .filter_map(|block| {
                block
                    .states
                    .iter()
                    .map(|state| state.id)
                    .min()
                    .map(|first_state| (first_state, block.id.clone()))
            })
            .collect();
        blocks_by_first_state.sort_by_key(|(first_state, _)| *first_state);
        let raw_ids: HashMap<_, _> = blocks_by_first_state
            .into_iter()
            .enumerate()
            .map(|(raw_id, (_, id))| {
                (
                    id,
                    i32::try_from(raw_id).expect("block registry raw id fits i32"),
                )
            })
            .collect();
        let mut by_id_pre: Vec<Option<Arc<BlockState>>> = (0..total_states).map(|_| None).collect();
        let mut by_name = HashMap::with_capacity(report.len());
        let mut by_key = HashMap::with_capacity(total_states);

        for raw in report {
            if by_name.contains_key(&raw.id) {
                return Err(RegistryError::DuplicateBlock(raw.id.clone()));
            }
            let schema: Vec<(String, Vec<String>)> = raw
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            let default = raw
                .states
                .iter()
                .find(|s| s.default)
                .map(|s| BlockStateId(s.id))
                .ok_or_else(|| RegistryError::NoDefault(raw.id.clone()))?;
            let states: Vec<BlockStateId> = raw.states.iter().map(|s| BlockStateId(s.id)).collect();
            let block = Arc::new(Block {
                id: raw.id.clone(),
                raw_id: *raw_ids
                    .get(&raw.id)
                    .expect("block with a default state has a raw id"),
                properties: schema,
                default,
                states,
            });
            by_name.insert(raw.id.clone(), Arc::clone(&block));

            for s in &raw.states {
                let props_in_order = props_in_schema_order(&block.properties, s);
                let state = Arc::new(BlockState {
                    block: Arc::clone(&block),
                    id: BlockStateId(s.id),
                    properties: props_in_order.clone(),
                });
                let idx = s.id as usize;
                if idx >= by_id_pre.len() {
                    by_id_pre.resize(idx + 1, None);
                }
                if by_id_pre[idx].is_some() {
                    return Err(RegistryError::DuplicateState(BlockStateId(s.id)));
                }
                by_id_pre[idx] = Some(state);
                if by_key
                    .insert((raw.id.clone(), props_in_order), BlockStateId(s.id))
                    .is_some()
                {
                    return Err(RegistryError::DuplicateState(BlockStateId(s.id)));
                }
            }
        }

        let by_id = by_id_pre
            .into_iter()
            .enumerate()
            .map(|(i, opt)| opt.ok_or(RegistryError::NotDense(i as u32)))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            by_id,
            by_name,
            by_key,
        })
    }

    #[must_use]
    pub fn by_id(&self, id: BlockStateId) -> Option<&BlockState> {
        self.by_id.get(id.0 as usize).map(Arc::as_ref)
    }

    #[must_use]
    pub fn block(&self, name: &Identifier) -> Option<&Block> {
        self.by_name.get(name).map(Arc::as_ref)
    }

    /// Resolve a state by block id + property values. The supplied
    /// `props` may be in any order; they are normalised against the
    /// block's schema before lookup.
    #[must_use]
    pub fn by_name_and_props(
        &self,
        name: &Identifier,
        props: &[(String, String)],
    ) -> Option<BlockStateId> {
        let block = self.by_name.get(name)?;
        let canonical: Vec<(String, String)> = block
            .properties
            .iter()
            .map(|(k, _)| {
                let v = props
                    .iter()
                    .find(|(pk, _)| pk == k)
                    .map(|(_, pv)| pv.clone())
                    .unwrap_or_default();
                (k.clone(), v)
            })
            .collect();
        self.by_key.get(&(name.clone(), canonical)).copied()
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// Iterator over every registered state, in id order.
    pub fn states(&self) -> impl Iterator<Item = &BlockState> {
        self.by_id.iter().map(Arc::as_ref)
    }
}

fn props_in_schema_order(
    schema: &[(String, Vec<String>)],
    state: &BlockStateReport,
) -> Vec<(String, String)> {
    schema
        .iter()
        .map(|(name, _allowed)| {
            let v = state.properties.get(name).cloned().unwrap_or_default();
            (name.clone(), v)
        })
        .collect()
}

#[cfg(test)]
#[path = "block_tests.rs"]
mod tests;
