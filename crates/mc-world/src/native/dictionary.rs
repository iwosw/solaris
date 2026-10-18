//! World-wide stable native IDs. Both indexes belong to the caller's state.redb
//! transaction; this module never commits or opens another database.
use std::collections::BTreeMap;

use mc_data::Identifier;
use mc_nbt::Tag;
use redb::{
    ReadOnlyTable, ReadTransaction, ReadableTable, ReadableTableMetadata, Table, TableDefinition,
    WriteTransaction,
};

use crate::anvil::chunk_nbt::block_state_to_palette_entry;
use crate::{BlockRegistry, BlockStateId};

use super::{NativeError, codec, db};

pub(super) const STATES: TableDefinition<u32, &[u8]> = TableDefinition::new("native_states");
pub(super) const IDS: TableDefinition<&[u8], u32> = TableDefinition::new("native_state_ids");

/// Creation only. Opening an existing world must use Reader::open, not create
/// missing dictionary tables in a write transaction.
pub(super) fn initialize(transaction: &WriteTransaction) -> Result<(), NativeError> {
    db(transaction.open_table(STATES))?;
    db(transaction.open_table(IDS))?;
    Ok(())
}

pub(super) trait Resolve {
    fn resolve(&self, id: u32, blocks: &BlockRegistry) -> Result<BlockStateId, NativeError>;
}

pub(super) struct Reader {
    states: ReadOnlyTable<u32, &'static [u8]>,
    ids: ReadOnlyTable<&'static [u8], u32>,
}

impl Reader {
    pub(super) fn open(transaction: &ReadTransaction) -> Result<Self, NativeError> {
        let states = db(transaction.open_table(STATES))?;
        let ids = db(transaction.open_table(IDS))?;
        if db(states.len())? != db(ids.len())? {
            return Err(NativeError::Invalid("dictionary indexes disagree"));
        }
        Ok(Self { states, ids })
    }
}

impl Resolve for Reader {
    fn resolve(&self, id: u32, blocks: &BlockRegistry) -> Result<BlockStateId, NativeError> {
        resolve(&self.states, &self.ids, id, blocks)
    }
}

pub(super) struct Writer<'txn> {
    states: Table<'txn, u32, &'static [u8]>,
    ids: Table<'txn, &'static [u8], u32>,
    next: u32,
    // A transaction-local runtime mapping, never a durable runtime ID.
    runtime: BTreeMap<BlockStateId, u32>,
}

impl<'txn> Writer<'txn> {
    /// Call after dictionary tables have been checked by the world-open path.
    pub(super) fn open(transaction: &'txn WriteTransaction) -> Result<Self, NativeError> {
        let states = db(transaction.open_table(STATES))?;
        let ids = db(transaction.open_table(IDS))?;
        if db(states.len())? != db(ids.len())? {
            return Err(NativeError::Invalid("dictionary indexes disagree"));
        }
        let next = match db(states.last())? {
            None => 0,
            Some((id, _)) => id
                .value()
                .checked_add(1)
                .ok_or(NativeError::Invalid("native dictionary exhausted"))?,
        };
        Ok(Self {
            states,
            ids,
            next,
            runtime: BTreeMap::new(),
        })
    }

    pub(super) fn intern(
        &mut self,
        state: BlockStateId,
        blocks: &BlockRegistry,
    ) -> Result<u32, NativeError> {
        if let Some(id) = self.runtime.get(&state) {
            return Ok(*id);
        }
        let bytes = canonical(state, blocks)?;
        let existing = db(self.ids.get(bytes.as_slice()))?.map(|id| id.value());
        let id = match existing {
            Some(id) => {
                if self.resolve(id, blocks)? != state {
                    return Err(NativeError::Invalid("dictionary reverse mapping disagrees"));
                }
                id
            }
            None => {
                let id = self.next;
                self.next = id
                    .checked_add(1)
                    .ok_or(NativeError::Invalid("native dictionary exhausted"))?;
                db(self.states.insert(id, bytes.as_slice()))?;
                db(self.ids.insert(bytes.as_slice(), id))?;
                id
            }
        };
        self.runtime.insert(state, id);
        Ok(id)
    }
}

impl Resolve for Writer<'_> {
    fn resolve(&self, id: u32, blocks: &BlockRegistry) -> Result<BlockStateId, NativeError> {
        resolve(&self.states, &self.ids, id, blocks)
    }
}

fn resolve(
    states: &impl ReadableTable<u32, &'static [u8]>,
    ids: &impl ReadableTable<&'static [u8], u32>,
    id: u32,
    blocks: &BlockRegistry,
) -> Result<BlockStateId, NativeError> {
    let bytes = db(states.get(id))?.ok_or(NativeError::Invalid("unknown native block state ID"))?;
    if db(ids.get(bytes.value()))?.is_none_or(|entry| entry.value() != id) {
        return Err(NativeError::Invalid("dictionary reverse mapping disagrees"));
    }
    let state = resolve_state(&codec::decode(bytes.value())?, blocks)?;
    if canonical(state, blocks)? != bytes.value() {
        return Err(NativeError::Invalid("noncanonical native block state"));
    }
    Ok(state)
}

fn canonical(state: BlockStateId, blocks: &BlockRegistry) -> Result<Vec<u8>, NativeError> {
    let mut tag = block_state_to_palette_entry(state, blocks)?;
    if let Tag::Compound(fields) = &mut tag {
        for (key, value) in fields {
            if key == "Properties"
                && let Tag::Compound(properties) = value
            {
                properties.sort_by(|first, second| first.0.cmp(&second.0));
            }
        }
    }
    codec::encode(&tag)
}

fn resolve_state(tag: &Tag, blocks: &BlockRegistry) -> Result<BlockStateId, NativeError> {
    let Tag::Compound(fields) = tag else {
        return Err(NativeError::Invalid("invalid dictionary state"));
    };
    if fields.len() != 1 && fields.len() != 2 {
        return Err(NativeError::Invalid("invalid dictionary state fields"));
    }
    let Some((name_key, Tag::String(name))) = fields.first() else {
        return Err(NativeError::Invalid("missing state name"));
    };
    if name_key != "Name" {
        return Err(NativeError::Invalid("missing state name"));
    }
    let name = Identifier::parse(name).map_err(|_| NativeError::Invalid("invalid state name"))?;
    let properties = match fields.get(1) {
        None => Vec::new(),
        Some((key, Tag::Compound(properties))) if key == "Properties" => properties
            .iter()
            .map(|(key, value)| match value {
                Tag::String(value) => Ok((key.clone(), value.clone())),
                _ => Err(NativeError::Invalid("invalid state property")),
            })
            .collect::<Result<Vec<_>, _>>()?,
        _ => return Err(NativeError::Invalid("invalid state properties")),
    };
    let id = blocks
        .by_name_and_props(&name, &properties)
        .ok_or(NativeError::Invalid("unknown stable block state"))?;
    let state = blocks.by_id(id).expect("resolved state");
    if state.properties.len() != properties.len()
        || !state.properties.iter().all(|property| {
            properties
                .iter()
                .filter(|candidate| *candidate == property)
                .count()
                == 1
        })
    {
        return Err(NativeError::Invalid("unknown or duplicate property"));
    }
    Ok(id)
}

#[cfg(test)]
#[path = "dictionary_tests.rs"]
mod tests;
