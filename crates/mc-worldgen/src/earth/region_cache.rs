//! One indexed derived container, never the native gameplay database.
use super::definition::{WorldIdentity, hash};
use super::region::{GeoNodeId, LATTICE_POINTS, REGION_VERSION, RegionPlan};
use redb::{Database, ReadableDatabase, ReadableTable, TableDefinition};
use std::path::Path;
use thiserror::Error;

const HEADER: TableDefinition<&str, u64> = TableDefinition::new("discardable_region_header");
const RECORDS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("derived_regions");
const ORDER: TableDefinition<u64, &[u8]> = TableDefinition::new("derived_eviction_order");
const SLOTS: TableDefinition<&[u8], u64> = TableDefinition::new("derived_order_slots");
const MARKER: u64 = 0x534f4c4445525631;
pub const MAX_RECORD_BYTES: usize = 4 * 1024 * 1024;
#[derive(Debug, Error)]
pub enum CacheError {
    #[error("derived cache database: {0}")]
    Database(redb::Error),
    #[error("derived cache IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("derived cache serialization: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid derived cache: {0}")]
    Invalid(&'static str),
}
fn db<T, E: Into<redb::Error>>(r: Result<T, E>) -> Result<T, CacheError> {
    r.map_err(|e| CacheError::Database(e.into()))
}
pub struct RegionCache {
    database: Database,
    quota_bytes: u64,
}
impl RegionCache {
    /// Creation is explicit and create-new. A native state path cannot be opened as cache.
    pub fn create(path: &Path, quota_bytes: u64, memory_bytes: usize) -> Result<Self, CacheError> {
        if quota_bytes == 0 || memory_bytes == 0 {
            return Err(CacheError::Invalid("zero cache budget"));
        }
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        let database = db(Database::builder()
            .set_cache_size(memory_bytes)
            .create_file(file))?;
        let txn = db(database.begin_write())?;
        {
            let mut header = db(txn.open_table(HEADER))?;
            db(header.insert("discardable", MARKER))?;
            db(header.insert("bytes", 0))?;
            db(header.insert("next", 0))?;
            db(txn.open_table(RECORDS))?;
            db(txn.open_table(ORDER))?;
            db(txn.open_table(SLOTS))?;
        }
        db(txn.commit())?;
        Ok(Self {
            database,
            quota_bytes,
        })
    }
    pub fn open(path: &Path, quota_bytes: u64, memory_bytes: usize) -> Result<Self, CacheError> {
        if quota_bytes == 0 || memory_bytes == 0 {
            return Err(CacheError::Invalid("zero cache budget"));
        }
        let database = db(Database::builder().set_cache_size(memory_bytes).open(path))?;
        {
            let txn = db(database.begin_read())?;
            let table = db(txn.open_table(HEADER))?;
            if db(table.get("discardable"))?.is_none_or(|v| v.value() != MARKER) {
                return Err(CacheError::Invalid(
                    "container is not explicitly discardable",
                ));
            }
        }
        let cache = Self {
            database,
            quota_bytes,
        };
        if cache.logical_bytes()? > quota_bytes {
            cache.clear()?;
        }
        Ok(cache)
    }
    pub fn get(
        &self,
        identity: WorldIdentity,
        node: GeoNodeId,
    ) -> Result<Option<RegionPlan>, CacheError> {
        let txn = db(self.database.begin_read())?;
        let records = db(txn.open_table(RECORDS))?;
        let key = key(identity, node);
        let Some(record) = db(records.get(key.as_slice()))? else {
            return Ok(None);
        };
        // Only this explicitly discardable record can become a cache miss on corruption.
        let bytes = record.value();
        if bytes.len() < 36 || bytes.len() > MAX_RECORD_BYTES {
            return Ok(None);
        }
        if bytes[..4] != REGION_VERSION.to_le_bytes() || bytes[4..36] != hash(&bytes[36..]) {
            return Ok(None);
        }
        let Ok(plan) = serde_json::from_slice::<RegionPlan>(&bytes[36..]) else {
            return Ok(None);
        };
        if plan.identity != identity
            || plan.node != node
            || plan.version != REGION_VERSION
            || plan.columns.len() != LATTICE_POINTS
            || plan.surface_m.len() != LATTICE_POINTS
            || plan.edges.len() != 4
            || plan.corners.len() != 4
        {
            return Ok(None);
        }
        Ok(Some(plan))
    }
    /// Logical encoded bytes are capped; redb page/MVCC overhead is measured separately.
    pub fn insert(&self, plan: &RegionPlan) -> Result<(), CacheError> {
        let mut bytes = vec![0u8; 36];
        serde_json::to_writer(&mut bytes, plan)?;
        let size = bytes.len();
        if size > MAX_RECORD_BYTES {
            return Err(CacheError::Invalid("record exceeds explicit region bound"));
        }
        if size as u64 > self.quota_bytes {
            return Ok(());
        }
        bytes[..4].copy_from_slice(&REGION_VERSION.to_le_bytes());
        let checksum = hash(&bytes[36..]);
        bytes[4..36].copy_from_slice(&checksum);
        let key = key(plan.identity, plan.node);
        let txn = db(self.database.begin_write())?;
        {
            let mut header = db(txn.open_table(HEADER))?;
            let mut records = db(txn.open_table(RECORDS))?;
            let mut order = db(txn.open_table(ORDER))?;
            let mut slots = db(txn.open_table(SLOTS))?;
            let mut used = db(header.get("bytes"))?
                .ok_or(CacheError::Invalid("missing cache byte count"))?
                .value();
            let next = db(header.get("next"))?
                .ok_or(CacheError::Invalid("missing cache sequence"))?
                .value();
            if let Some(old) = db(records.remove(key.as_slice()))? {
                used = used
                    .checked_sub(old.value().len() as u64)
                    .ok_or(CacheError::Invalid("cache byte count underflow"))?;
            }
            // A replacement removes its old order slot, so metadata is also quota-bounded.
            if let Some(slot) = db(slots.remove(key.as_slice()))? {
                db(order.remove(slot.value()))?;
            }
            while used
                .checked_add(size as u64)
                .is_none_or(|v| v > self.quota_bytes)
            {
                let first = {
                    let mut iter = db(order.iter())?;
                    let Some(row) = iter.next() else {
                        return Err(CacheError::Invalid("cache count without records"));
                    };
                    let (slot, k) = db(row)?;
                    let key: [u8; 53] = k
                        .value()
                        .try_into()
                        .map_err(|_| CacheError::Invalid("invalid eviction key"))?;
                    (slot.value(), key)
                };
                if let Some(old) = db(records.remove(first.1.as_slice()))? {
                    used = used
                        .checked_sub(old.value().len() as u64)
                        .ok_or(CacheError::Invalid("cache eviction underflow"))?;
                }
                db(order.remove(first.0))?;
                db(slots.remove(first.1.as_slice()))?;
            }
            db(records.insert(key.as_slice(), bytes.as_slice()))?;
            db(order.insert(next, key.as_slice()))?;
            db(slots.insert(key.as_slice(), next))?;
            db(header.insert("bytes", used + size as u64))?;
            db(header.insert(
                "next",
                next.checked_add(1)
                    .ok_or(CacheError::Invalid("cache sequence exhausted"))?,
            ))?;
        }
        db(txn.commit())?;
        Ok(())
    }
    pub fn logical_bytes(&self) -> Result<u64, CacheError> {
        let txn = db(self.database.begin_read())?;
        let header = db(txn.open_table(HEADER))?;
        Ok(db(header.get("bytes"))?
            .ok_or(CacheError::Invalid("missing cache byte count"))?
            .value())
    }
    pub fn clear(&self) -> Result<(), CacheError> {
        let txn = db(self.database.begin_write())?;
        db(txn.delete_table(RECORDS))?;
        db(txn.delete_table(ORDER))?;
        db(txn.delete_table(SLOTS))?;
        {
            db(txn.open_table(RECORDS))?;
            db(txn.open_table(ORDER))?;
            db(txn.open_table(SLOTS))?;
            let mut h = db(txn.open_table(HEADER))?;
            db(h.insert("bytes", 0))?;
        }
        db(txn.commit())?;
        Ok(())
    }
}
fn key(identity: WorldIdentity, node: GeoNodeId) -> [u8; 53] {
    let mut key = [0u8; 53];
    key[..32].copy_from_slice(&identity.bytes());
    key[32..36].copy_from_slice(&REGION_VERSION.to_le_bytes());
    key[36..44].copy_from_slice(&node.root.to_le_bytes());
    key[44] = node.level;
    key[45..49].copy_from_slice(&node.x.to_le_bytes());
    key[49..].copy_from_slice(&node.z.to_le_bytes());
    key
}
#[cfg(test)]
#[path = "region_cache_tests.rs"]
mod tests;
