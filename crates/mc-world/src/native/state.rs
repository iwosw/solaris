//! Durable after-images supplied by the existing entity owner, not a live entity store.
//! A single identity key makes migration one replacement, never remove/add writes.
use mc_nbt::Tag;
use redb::{ReadableTable, ReadableTableMetadata, TableDefinition};

use super::{NativeError, NativeStore, codec, db};
use crate::ChunkPos;

pub(super) const ENTITIES: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("persistent_entities");
pub(super) const INITIALIZED: TableDefinition<&[u8], &[u8]> =
    TableDefinition::new("generated_slots");
pub(super) const ENTITY_LSN: TableDefinition<&[u8], u64> = TableDefinition::new("entity_lsn");

/// After-image of a persistent entity from its owner. `None` is a durable tombstone.
/// Generated slots are world-identity-derived descriptors, not random spawn attempts.
#[derive(Clone)]
pub struct NativeEntitySnapshot {
    pub identity: [u8; 16],
    pub dimension: u32,
    pub position: ChunkPos,
    pub generated_slot: Option<[u8; 32]>,
    pub journal_lsn: u64,
    pub payload: Option<Tag>,
}

pub(super) fn encode(entity: &NativeEntitySnapshot) -> Result<Vec<u8>, NativeError> {
    encode_at_lsn(entity, entity.journal_lsn)
}

pub(super) fn encode_at_lsn(
    entity: &NativeEntitySnapshot,
    journal_lsn: u64,
) -> Result<Vec<u8>, NativeError> {
    if entity
        .payload
        .as_ref()
        .is_some_and(|tag| !matches!(tag, Tag::Compound(_)))
    {
        return Err(NativeError::Invalid(
            "entity owner payload must be a compound",
        ));
    }
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&entity.dimension.to_be_bytes());
    bytes.extend_from_slice(&entity.position.x.to_be_bytes());
    bytes.extend_from_slice(&entity.position.z.to_be_bytes());
    bytes.extend_from_slice(&journal_lsn.to_be_bytes());
    bytes.push(u8::from(entity.generated_slot.is_some()));
    if let Some(slot) = entity.generated_slot {
        bytes.extend_from_slice(&slot);
    }
    bytes.push(u8::from(entity.payload.is_some()));
    if let Some(payload) = &entity.payload {
        super::checkpoint_validation::nbt(payload)?;
        mc_nbt::write_named(&mut bytes, "", payload)?;
        if bytes.len() > 16 * 1024 * 1024 {
            return Err(NativeError::Invalid(
                "native entity record exceeds byte budget",
            ));
        }
    }
    Ok(bytes)
}

pub(super) fn decode(
    identity: [u8; 16],
    bytes: &[u8],
) -> Result<NativeEntitySnapshot, NativeError> {
    let mut remaining = bytes;
    fn take<const N: usize>(bytes: &mut &[u8]) -> Result<[u8; N], NativeError> {
        let (value, rest) = bytes
            .split_at_checked(N)
            .ok_or(NativeError::Invalid("truncated entity record"))?;
        *bytes = rest;
        Ok(value.try_into().expect("checked length"))
    }
    let dimension = u32::from_be_bytes(take(&mut remaining)?);
    let x = i32::from_be_bytes(take(&mut remaining)?);
    let z = i32::from_be_bytes(take(&mut remaining)?);
    let journal_lsn = u64::from_be_bytes(take(&mut remaining)?);
    let generated_slot = match take::<1>(&mut remaining)?[0] {
        0 => None,
        1 => Some(take(&mut remaining)?),
        _ => return Err(NativeError::Invalid("invalid generated slot flag")),
    };
    let payload = match take::<1>(&mut remaining)?[0] {
        0 if remaining.is_empty() => None,
        1 => Some(codec::decode(remaining)?),
        _ => return Err(NativeError::Invalid("invalid entity payload flag")),
    };
    let entity = NativeEntitySnapshot {
        identity,
        dimension,
        position: ChunkPos { x, z },
        generated_slot,
        journal_lsn,
        payload,
    };
    if entity
        .payload
        .as_ref()
        .is_some_and(|tag| !matches!(tag, Tag::Compound(_)))
    {
        return Err(NativeError::Invalid("invalid entity owner record"));
    }
    Ok(entity)
}

pub(super) fn initialize(transaction: &redb::WriteTransaction) -> Result<(), NativeError> {
    db(transaction.open_table(ENTITIES))?;
    db(transaction.open_table(INITIALIZED))?;
    db(transaction.open_table(ENTITY_LSN))?;
    Ok(())
}

/// Inserts one pre-encoded entity row with slot/tombstone validation. Batch
/// bounds stay with the caller: the WAL path keeps its 256-participant/64 MiB
/// limits while checkpoints stream without an aggregate budget. Every row is
/// encoded exactly once by the caller.
fn insert_encoded(
    entities: &mut redb::Table<'_, &[u8], &[u8]>,
    initialized: &mut redb::Table<'_, &[u8], &[u8]>,
    applied: &mut redb::Table<'_, &[u8], u64>,
    entity: &NativeEntitySnapshot,
    journal_lsn: u64,
    bytes: &[u8],
) -> Result<(), NativeError> {
    let identity = entity.identity;
    let generated_slot = entity.generated_slot;
    if let Some(previous) = db(entities.get(identity.as_slice()))? {
        let previous = decode(identity, previous.value())?;
        if previous.generated_slot != generated_slot {
            return Err(NativeError::Invalid("entity generated slot cannot change"));
        }
        if previous.payload.is_none() && entity.payload.is_some() {
            return Err(NativeError::Invalid(
                "persistent entity tombstone cannot respawn",
            ));
        }
    }
    if let Some(slot) = generated_slot {
        // A slot belongs to its world, not its current dimension or position.
        if let Some(existing) = db(initialized.get(slot.as_slice()))? {
            if existing.value() != identity {
                return Err(NativeError::Invalid("generated slot already initialized"));
            }
        } else {
            db(initialized.insert(slot.as_slice(), identity.as_slice()))?;
        }
    }
    db(entities.insert(identity.as_slice(), bytes))?;
    db(applied.insert(identity.as_slice(), journal_lsn))?;
    Ok(())
}

/// Replay retains both the per-row and admitted global checkpoint fences.
pub(super) fn write(
    transaction: &redb::WriteTransaction,
    snapshots: &[NativeEntitySnapshot],
    floor: Option<u64>,
) -> Result<usize, NativeError> {
    if snapshots.len() > 256 {
        return Err(NativeError::Invalid(
            "native entity batch exceeds 256 participants",
        ));
    }
    let mut encoded_bytes = 0_usize;
    let mut entities = db(transaction.open_table(ENTITIES))?;
    let mut initialized = db(transaction.open_table(INITIALIZED))?;
    let mut applied = db(transaction.open_table(ENTITY_LSN))?;
    let mut seen = std::collections::BTreeSet::new();
    let mut written = 0;
    for entity in snapshots {
        if !seen.insert(entity.identity) {
            return Err(NativeError::Invalid("duplicate entity participant"));
        }
        if entity.journal_lsn == 0 {
            return Err(NativeError::Invalid(
                "entity snapshot must have a journal decision",
            ));
        }
        if floor.is_some_and(|tick| entity.journal_lsn <= tick) {
            continue;
        }
        if db(applied.get(entity.identity.as_slice()))?
            .is_some_and(|lsn| lsn.value() >= entity.journal_lsn)
        {
            continue;
        }
        let bytes = encode(entity)?;
        encoded_bytes += bytes.len();
        if encoded_bytes > 64 * 1024 * 1024 {
            return Err(NativeError::Invalid(
                "native entity batch exceeds byte budget",
            ));
        }
        insert_encoded(
            &mut entities,
            &mut initialized,
            &mut applied,
            entity,
            entity.journal_lsn,
            bytes.as_slice(),
        )?;
        written += 1;
    }
    Ok(written)
}

impl NativeStore {
    /// Restores the existing entity owner's checkpoint, including consumed-slot
    /// tombstones. This never activates content or creates another entity owner.
    pub fn load_entity_checkpoint(
        &self,
    ) -> Result<Option<(Tag, Vec<NativeEntitySnapshot>)>, NativeError> {
        use redb::ReadableDatabase;
        let transaction = db(self.database.begin_read())?;
        let header = db(transaction.open_table(super::HEADER))?;
        let entities = db(transaction.open_table(ENTITIES))?;
        let metadata = db(header.get("entity_checkpoint"))?;
        let Some(metadata) = metadata else {
            if db(entities.len())? != 0 {
                return Err(NativeError::Invalid("entity checkpoint metadata missing"));
            }
            return Ok(None);
        };
        let metadata = codec::decode(metadata.value())?;
        if !matches!(metadata, Tag::Compound(_)) {
            return Err(NativeError::Invalid(
                "entity checkpoint metadata must be compound",
            ));
        }
        let applied = db(transaction.open_table(ENTITY_LSN))?;
        let initialized = db(transaction.open_table(INITIALIZED))?;
        let mut snapshots = Vec::new();
        for record in db(entities.iter())? {
            let (identity, record) = db(record)?;
            let identity: [u8; 16] = identity
                .value()
                .try_into()
                .map_err(|_| NativeError::Invalid("invalid entity identity"))?;
            let entity = decode(identity, record.value())?;
            if db(applied.get(identity.as_slice()))?
                .is_none_or(|lsn| lsn.value() != entity.journal_lsn)
            {
                return Err(NativeError::Invalid(
                    "entity checkpoint participant mismatch",
                ));
            }
            if let Some(slot) = entity.generated_slot
                && db(initialized.get(slot.as_slice()))?
                    .is_none_or(|record| record.value() != identity)
            {
                return Err(NativeError::Invalid("entity checkpoint slot mismatch"));
            }
            snapshots.push(entity);
        }
        Ok(Some((metadata, snapshots)))
    }

    /// Immediate, atomic after-images from the existing owner after its WAL
    /// barrier. Omitted live entities become durable tombstones; slots never reset.
    /// The owner remains responsible for checkpointing/truncating its journal.
    pub fn save_entity_checkpoint(
        &self,
        metadata: Tag,
        snapshots: &[NativeEntitySnapshot],
    ) -> Result<(), NativeError> {
        if !matches!(metadata, Tag::Compound(_)) {
            return Err(NativeError::Invalid(
                "entity checkpoint metadata must be compound",
            ));
        }
        let metadata = codec::encode(&metadata)?;
        let mut transaction = db(self.database.begin_write())?;
        db(transaction.set_durability(redb::Durability::Immediate))?;
        let mut identities = std::collections::BTreeSet::new();
        let mut tombstones = Vec::new();
        for entity in snapshots {
            if !identities.insert(entity.identity) {
                return Err(NativeError::Invalid("duplicate entity checkpoint identity"));
            }
        }
        let mut latest_lsn = 0_u64;
        {
            let entities = db(transaction.open_table(ENTITIES))?;
            for record in db(entities.iter())? {
                let (identity, record) = db(record)?;
                let identity: [u8; 16] = identity
                    .value()
                    .try_into()
                    .map_err(|_| NativeError::Invalid("invalid entity identity"))?;
                let previous = decode(identity, record.value())?;
                latest_lsn = latest_lsn.max(previous.journal_lsn);
                if !identities.contains(&identity) && previous.payload.is_some() {
                    tombstones.push(NativeEntitySnapshot {
                        payload: None,
                        journal_lsn: 0,
                        ..previous
                    });
                }
            }
            let header = db(transaction.open_table(super::HEADER))?;
            if let Some(record) = db(header.get("entity_checkpoint_lsn"))? {
                let bytes: [u8; 8] = record
                    .value()
                    .try_into()
                    .map_err(|_| NativeError::Invalid("invalid entity checkpoint LSN"))?;
                latest_lsn = latest_lsn.max(u64::from_be_bytes(bytes));
            }
        }
        let checkpoint_lsn = latest_lsn
            .checked_add(1)
            .ok_or(NativeError::Invalid("entity checkpoint LSN exhausted"))?;
        // Count-scalable: each row is validated and encoded exactly once at the
        // checkpoint LSN; tables stream all rows with no aggregate byte budget.
        // The 256-participant/64 MiB fence stays on the WAL `write` path only.
        {
            let mut entities = db(transaction.open_table(ENTITIES))?;
            let mut initialized = db(transaction.open_table(INITIALIZED))?;
            let mut applied = db(transaction.open_table(ENTITY_LSN))?;
            for entity in snapshots.iter().chain(tombstones.iter()) {
                let bytes = encode_at_lsn(entity, checkpoint_lsn)?;
                insert_encoded(
                    &mut entities,
                    &mut initialized,
                    &mut applied,
                    entity,
                    checkpoint_lsn,
                    bytes.as_slice(),
                )?;
            }
        }
        {
            let mut header = db(transaction.open_table(super::HEADER))?;
            db(header.insert("entity_checkpoint", metadata.as_slice()))?;
            db(header.insert(
                "entity_checkpoint_lsn",
                checkpoint_lsn.to_be_bytes().as_slice(),
            ))?;
        }
        db(transaction.commit())
    }

    /// Returns the latest owner after-image, including tombstones. Never activates slots.
    pub fn entity(&self, identity: [u8; 16]) -> Result<Option<NativeEntitySnapshot>, NativeError> {
        use redb::ReadableDatabase;
        let transaction = db(self.database.begin_read())?;
        let entities = db(transaction.open_table(ENTITIES))?;
        let applied = db(transaction.open_table(ENTITY_LSN))?;
        let initialized = db(transaction.open_table(INITIALIZED))?;
        let record = db(entities.get(identity.as_slice()))?;
        record
            .map(|record| {
                let entity = decode(identity, record.value())?;
                if db(applied.get(identity.as_slice()))?
                    .is_none_or(|lsn| lsn.value() != entity.journal_lsn)
                {
                    return Err(NativeError::Invalid("entity/applied LSN mismatch"));
                }
                if let Some(slot) = entity.generated_slot
                    && db(initialized.get(slot.as_slice()))?
                        .is_none_or(|record| record.value() != identity)
                {
                    return Err(NativeError::Invalid("entity/generated slot mismatch"));
                }
                Ok(entity)
            })
            .transpose()
    }

    pub fn generated_slot(&self, slot: [u8; 32]) -> Result<Option<[u8; 16]>, NativeError> {
        use redb::ReadableDatabase;
        let transaction = db(self.database.begin_read())?;
        let initialized = db(transaction.open_table(INITIALIZED))?;
        let entities = db(transaction.open_table(ENTITIES))?;
        let applied = db(transaction.open_table(ENTITY_LSN))?;
        db(initialized.get(slot.as_slice()))?
            .map(|record| {
                let identity: [u8; 16] = record
                    .value()
                    .try_into()
                    .map_err(|_| NativeError::Invalid("invalid generated slot identity"))?;
                let record = db(entities.get(identity.as_slice()))?
                    .ok_or(NativeError::Invalid("initialized entity record missing"))?;
                let entity = decode(identity, record.value())?;
                if entity.generated_slot != Some(slot)
                    || db(applied.get(identity.as_slice()))?
                        .is_none_or(|lsn| lsn.value() != entity.journal_lsn)
                {
                    return Err(NativeError::Invalid("generated slot/entity mismatch"));
                }
                Ok(identity)
            })
            .transpose()
    }
}
