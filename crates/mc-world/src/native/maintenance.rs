//! Offline storage maintenance over a stopped world: pruning chunks that nobody
//! ever changed or lived in, repacking rows written before compression, and
//! returning free pages to the file system. The caller holds the world lease;
//! nothing here is reachable from the running tick.
use std::collections::HashSet;

use mc_nbt::Tag;
use redb::{ReadableDatabase, ReadableTable};

use super::{
    APPLIED, GENERATED, IMPORTED, METADATA, NativeError, NativeStore, OVERLAYS, codec, db, pack,
};

/// Keys removed or rewritten per durable transaction.
const BATCH: usize = 2048;
const INHABITED: &str = "InhabitedTime";
const JOURNAL_LSN: &str = "SolarisJournalLsn";

/// Which untouched chunks of one dimension to forget. Chunks inside the
/// inclusive `keep_min..=keep_max` rectangle are never pruned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativePruneRequest {
    pub dimension: u32,
    pub keep_min: [i32; 2],
    pub keep_max: [i32; 2],
    /// Prune only chunks whose effective inhabited time is at most this many ticks.
    pub max_inhabited_ticks: i64,
    /// Count what would be pruned without writing.
    pub dry_run: bool,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NativePruneReport {
    /// Stored baseline chunks of the dimension.
    pub examined: u64,
    pub pruned: u64,
    pub kept_inside_rectangle: u64,
    pub kept_inhabited: u64,
    /// Block, entity, overlay or side-state differences from generation.
    pub kept_changed: u64,
    /// Baselines not recorded as generator-created (Anvil imports, rows
    /// written before provenance existed): the generator cannot recreate them.
    pub kept_not_generated: u64,
}

fn inhabited_ticks(root: &Tag) -> i64 {
    let Tag::Compound(fields) = root else {
        return 0;
    };
    fields
        .iter()
        .find_map(|(key, tag)| match tag {
            Tag::Long(value) if key == INHABITED => Some(*value),
            _ => None,
        })
        .unwrap_or(0)
}

/// Equal chunk content once the capture stamps (journal LSN, inhabited clock)
/// and the baseline-only `block_states` payload are set aside.
fn same_content(metadata: Tag, baseline: Tag) -> Result<bool, NativeError> {
    let (mut metadata, mut baseline) = (metadata, baseline);
    codec::compound_mut(&mut metadata)?.retain(|(key, _)| key != JOURNAL_LSN && key != INHABITED);
    codec::compound_mut(&mut baseline)?.retain(|(key, _)| key != INHABITED);
    for section in codec::sections_mut(&mut baseline)? {
        codec::compound_mut(section)?.retain(|(key, _)| key != "block_states");
    }
    Ok(metadata == baseline)
}

fn chunk_of(key: &[u8]) -> Result<(u32, i32, i32), NativeError> {
    let key: &[u8; 12] = key
        .get(..12)
        .and_then(|key| key.try_into().ok())
        .ok_or(NativeError::Invalid("invalid chunk key"))?;
    let flip = |bytes: &[u8]| {
        (u32::from_be_bytes(bytes.try_into().expect("4 bytes")) ^ 0x8000_0000) as i32
    };
    Ok((
        u32::from_be_bytes(key[..4].try_into().expect("4 bytes")),
        flip(&key[4..8]),
        flip(&key[8..]),
    ))
}

impl NativeStore {
    /// Forget chunks outside the keep rectangle whose baseline the Solaris
    /// generator created (listed in `GENERATED`) and that still equal it: no
    /// overlay sections, no persistent entity rows (including tombstones of
    /// consumed generated slots), no side-state difference from the baseline,
    /// and an inhabited clock at or below the limit. Their baseline, metadata,
    /// watermark and provenance rows go together; the next visit generates the
    /// chunk again with the current generator. Baselines imported from Anvil, or
    /// stored before provenance was recorded, are never pruned: the generator
    /// cannot recreate them. Chunks without a stored baseline are kept.
    pub fn prune_pristine(
        &self,
        request: &NativePruneRequest,
    ) -> Result<NativePruneReport, NativeError> {
        let mut report = NativePruneReport::default();
        let mut doomed: Vec<[u8; 12]> = Vec::new();
        {
            let transaction = db(self.database.begin_read())?;
            let occupied = occupied_chunks(&transaction, request.dimension)?;
            let overlays = db(transaction.open_table(OVERLAYS))?;
            let metadata = db(transaction.open_table(METADATA))?;
            let imported = db(transaction.open_table(IMPORTED))?;
            let generated = match transaction.open_table(GENERATED) {
                Ok(table) => Some(table),
                Err(redb::TableError::TableDoesNotExist(_)) => None,
                Err(error) => return Err(NativeError::Database(error.into())),
            };
            let mut overlay_chunks = HashSet::new();
            for row in db(overlays.iter())? {
                let (key, _) = db(row)?;
                let (dimension, x, z) = chunk_of(key.value())?;
                if dimension == request.dimension {
                    overlay_chunks.insert((x, z));
                }
            }
            for row in db(imported.iter())? {
                let (key, baseline) = db(row)?;
                let (dimension, x, z) = chunk_of(key.value())?;
                if dimension != request.dimension {
                    continue;
                }
                report.examined += 1;
                if (request.keep_min[0]..=request.keep_max[0]).contains(&x)
                    && (request.keep_min[1]..=request.keep_max[1]).contains(&z)
                {
                    report.kept_inside_rectangle += 1;
                    continue;
                }
                let created_by_generator = match &generated {
                    Some(table) => db(table.get(key.value()))?.is_some(),
                    None => false,
                };
                if !created_by_generator {
                    report.kept_not_generated += 1;
                    continue;
                }
                if occupied.contains(&(x, z)) || overlay_chunks.contains(&(x, z)) {
                    report.kept_changed += 1;
                    continue;
                }
                let baseline = codec::decode(baseline.value())?;
                let stored = db(metadata.get(key.value()))?;
                let (effective_ticks, unchanged) = match &stored {
                    None => (inhabited_ticks(&baseline), true),
                    Some(record) => {
                        let record = codec::decode(record.value())?;
                        let ticks = inhabited_ticks(&record);
                        (ticks, same_content(record, baseline)?)
                    }
                };
                if !unchanged {
                    report.kept_changed += 1;
                } else if effective_ticks > request.max_inhabited_ticks {
                    report.kept_inhabited += 1;
                } else {
                    report.pruned += 1;
                    doomed.push(key.value().try_into().expect("12-byte chunk key"));
                }
            }
        }
        if request.dry_run {
            return Ok(report);
        }
        for batch in doomed.chunks(BATCH) {
            let mut transaction = db(self.database.begin_write())?;
            db(transaction.set_durability(redb::Durability::Immediate))?;
            {
                let mut imported = db(transaction.open_table(IMPORTED))?;
                let mut metadata = db(transaction.open_table(METADATA))?;
                let mut applied = db(transaction.open_table(APPLIED))?;
                let mut generated = db(transaction.open_table(GENERATED))?;
                for key in batch {
                    db(imported.remove(key.as_slice()))?;
                    db(metadata.remove(key.as_slice()))?;
                    db(applied.remove(key.as_slice()))?;
                    db(generated.remove(key.as_slice()))?;
                }
            }
            db(transaction.commit())?;
        }
        Ok(report)
    }

    /// Rewrite baseline and metadata rows stored as plain NBT in the compressed
    /// form. Every row is fully decoded first, so a corrupt row aborts the run
    /// instead of being copied. Returns how many rows were rewritten.
    pub fn repack_rows(&self) -> Result<u64, NativeError> {
        let mut rewritten = 0;
        for table in [IMPORTED, METADATA] {
            let mut after: Option<Vec<u8>> = None;
            loop {
                let mut batch: Vec<(Vec<u8>, Vec<u8>)> = Vec::new();
                let mut last = None;
                {
                    let transaction = db(self.database.begin_read())?;
                    let rows = db(transaction.open_table(table))?;
                    let range = match &after {
                        Some(key) => db(rows.range::<&[u8]>((
                            std::ops::Bound::Excluded(key.as_slice()),
                            std::ops::Bound::Unbounded,
                        )))?,
                        None => db(rows.range::<&[u8]>(..))?,
                    };
                    for row in range {
                        let (key, value) = db(row)?;
                        last = Some(key.value().to_vec());
                        let value = value.value();
                        if !pack::is_packed(value) {
                            codec::decode(value)?;
                            let packed = pack::pack(value.to_vec())?;
                            if pack::is_packed(&packed) {
                                batch.push((key.value().to_vec(), packed));
                            }
                        }
                        if batch.len() == BATCH {
                            break;
                        }
                    }
                }
                let Some(last) = last else { break };
                if !batch.is_empty() {
                    let mut transaction = db(self.database.begin_write())?;
                    db(transaction.set_durability(redb::Durability::Immediate))?;
                    {
                        let mut rows = db(transaction.open_table(table))?;
                        for (key, value) in &batch {
                            db(rows.insert(key.as_slice(), value.as_slice()))?;
                        }
                    }
                    db(transaction.commit())?;
                    rewritten += batch.len() as u64;
                }
                after = Some(last);
            }
        }
        Ok(rewritten)
    }

    /// Return free pages to the file system. Offline only; consumes the store
    /// because compaction needs the only handle on the database.
    pub fn compact(mut self) -> Result<(), NativeError> {
        while self
            .database
            .compact()
            .map_err(|error| NativeError::Database(error.into()))?
        {}
        Ok(())
    }
}

/// Chunks of `dimension` that own a persistent entity row, tombstones included.
fn occupied_chunks(
    transaction: &redb::ReadTransaction,
    dimension: u32,
) -> Result<HashSet<(i32, i32)>, NativeError> {
    let entities = db(transaction.open_table(super::state::ENTITIES))?;
    let mut occupied = HashSet::new();
    for row in db(entities.iter())? {
        let (_, record) = db(row)?;
        let record = record.value();
        let header: &[u8; 12] = record
            .get(..12)
            .and_then(|header| header.try_into().ok())
            .ok_or(NativeError::Invalid("truncated entity record"))?;
        if u32::from_be_bytes(header[..4].try_into().expect("4 bytes")) == dimension {
            occupied.insert((
                i32::from_be_bytes(header[4..8].try_into().expect("4 bytes")),
                i32::from_be_bytes(header[8..].try_into().expect("4 bytes")),
            ));
        }
    }
    Ok(occupied)
}

#[cfg(test)]
#[path = "maintenance_tests.rs"]
mod tests;
