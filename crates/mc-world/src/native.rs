//! Native persistent after-images over an immutable compact-profile baseline.
//! Owns durable bytes, never a second resident authority. Runtime checkpoint work
//! executes off-thread through the native checkpoint transaction.

mod block_log;
mod checkpoint;
mod checkpoint_validation;
mod codec;
mod dictionary;
mod maintenance;
mod pack;
mod state;
mod ticks;
pub use checkpoint::{
    NativeCheckpoint, NativeCheckpointImage, NativeCheckpointIo, NativeCheckpointReader,
    NativeDomainRecord, NativeGenerationContract, NativePlayerSnapshot,
};
pub use maintenance::{NativePruneReport, NativePruneRequest};
#[cfg(test)]
mod checkpoint_tests;
#[cfg(test)]
mod pack_tests;
#[cfg(test)]
mod section_dirty_tests;
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
pub use state::NativeEntitySnapshot;
#[cfg(test)]
mod state_tests;
#[cfg(test)]
mod tests;

use std::fs::OpenOptions;
use std::path::Path;

use redb::{
    Database, Durability, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use thiserror::Error;

use crate::anvil::chunk_nbt::chunk_from_nbt_with_items_at_position_at_tick;
use crate::{BlockRegistry, Chunk, ChunkPos};

const HEADER: TableDefinition<&str, &[u8]> = TableDefinition::new("world");
const IMPORTED: TableDefinition<&[u8], &[u8]> = TableDefinition::new("imported_chunks");
const OVERLAYS: TableDefinition<&[u8], &[u8]> = TableDefinition::new("section_overlays");
const METADATA: TableDefinition<&[u8], &[u8]> = TableDefinition::new("chunk_metadata");
const APPLIED: TableDefinition<&[u8], u64> = TableDefinition::new("applied_lsn");
/// Baselines the Solaris generator produced (pregeneration, LOD fill, lazy
/// checkpoint bases). Anvil imports are not listed. Rows from builds before this
/// table existed are unlisted too, so offline pruning never treats them as
/// regenerable. Created on first use in an existing world; absent means empty.
const GENERATED: TableDefinition<&[u8], ()> = TableDefinition::new("generated_baselines");
/// Independent of the unchanged vanilla-like WORLDGEN_REVISION.
pub const NATIVE_PROTOTYPE_SCHEMA: u32 = 3;
const DB_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug, Error)]
pub enum NativeError {
    #[error("native database: {0}")]
    Database(redb::Error),
    #[error("native IO: {0}")]
    Io(#[from] std::io::Error),
    #[error("native NBT: {0}")]
    Nbt(#[from] mc_nbt::NbtError),
    #[error("native chunk codec: {0}")]
    Chunk(#[from] crate::anvil::ChunkNbtError),
    #[error("invalid native record: {0}")]
    Invalid(&'static str),
    #[error("native source: {0}")]
    Source(String),
    #[error("unsupported native schema {0}; explicit offline conversion required")]
    UnsupportedSchema(u32),
}

fn db<T, E: Into<redb::Error>>(result: Result<T, E>) -> Result<T, NativeError> {
    result.map_err(|error| NativeError::Database(error.into()))
}

fn read_only_database(
    path: &Path,
    cache_bytes: usize,
) -> Result<redb::ReadOnlyDatabase, NativeError> {
    Database::builder()
        .set_cache_size(cache_bytes)
        .open_read_only(path)
        .map_err(|error| match error {
            redb::DatabaseError::RepairAborted => NativeError::Source(
                "world was not shut down cleanly; start and stop the server once to repair it, then export"
                    .into(),
            ),
            error => NativeError::Database(error.into()),
        })
}

/// A bounded off-thread snapshot, borrowed from the existing world owner.
/// `base` must come from this world's source resolver, not from the live view.
pub struct NativeChunkSnapshot<'a> {
    pub dimension: u32,
    pub base: &'a Chunk,
    pub effective: &'a Chunk,
    /// World checkpoints only: block sections (bit = index from the geometry's
    /// `min_y`) whose states may differ from the stored overlay. Other sections
    /// are neither examined nor rewritten, so the owner must include every
    /// section edited since the stored image. `None` examines every section;
    /// replay always does.
    pub dirty_sections: Option<u32>,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct NativeRecordCounts {
    pub imported_chunks: u64,
    pub overlay_sections: u64,
    pub metadata_chunks: u64,
    pub applied_chunks: u64,
    pub persistent_entities: u64,
    pub initialized_slots: u64,
    pub stable_states: u64,
}

pub struct NativeStore {
    database: Database,
    identity: [u8; 32],
}

pub struct NativeReadOnlyStore {
    database: redb::ReadOnlyDatabase,
}

#[derive(Debug, PartialEq, Eq)]
pub struct NativeBlockOrigin {
    pub origin: &'static str,
    pub native_state_id: Option<u32>,
    pub applied_lsn: u64,
}

fn block_origin(
    transaction: &redb::ReadTransaction,
    dimension: u32,
    position: crate::BlockPos,
) -> Result<NativeBlockOrigin, NativeError> {
    let chunk = ChunkPos {
        x: position.x.div_euclid(16),
        z: position.z.div_euclid(16),
    };
    let overlays = db(transaction.open_table(OVERLAYS))?;
    let applied = db(transaction.open_table(APPLIED))?;
    let key = section_key(dimension, chunk, position.y.div_euclid(16));
    let index = (position.y.rem_euclid(16) * 256
        + position.z.rem_euclid(16) * 16
        + position.x.rem_euclid(16)) as usize;
    let state = db(overlays.get(key.as_slice()))?
        .map(|record| codec::inspect_state(record.value(), index))
        .transpose()?
        .flatten();
    let (origin, native_state_id) = match state {
        None => ("base", None),
        Some((1, id)) => ("sparse_override", Some(id)),
        Some((2, id)) => ("dense_replacement", Some(id)),
        Some(_) => return Err(NativeError::Invalid("unknown section representation")),
    };
    let applied_lsn =
        db(applied.get(chunk_key(dimension, chunk).as_slice()))?.map_or(0, |lsn| lsn.value());
    Ok(NativeBlockOrigin {
        origin,
        native_state_id,
        applied_lsn,
    })
}

impl NativeReadOnlyStore {
    pub fn open_with_cache(
        path: &Path,
        identity: [u8; 32],
        cache_bytes: usize,
    ) -> Result<Self, NativeError> {
        if cache_bytes == 0 {
            return Err(NativeError::Invalid("database cache must be positive"));
        }
        let database = read_only_database(path, cache_bytes)?;
        validate_header(&db(database.begin_read())?, identity)?;
        Ok(Self { database })
    }

    pub fn block_origin(
        &self,
        dimension: u32,
        position: crate::BlockPos,
    ) -> Result<NativeBlockOrigin, NativeError> {
        block_origin(&db(self.database.begin_read())?, dimension, position)
    }

    pub fn compose(
        &self,
        dimension: u32,
        base: &Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
    ) -> Result<Chunk, NativeError> {
        compose(
            &db(self.database.begin_read())?,
            dimension,
            base,
            blocks,
            items,
            chest_loot,
        )
    }

    pub fn resolve_base(
        &self,
        dimension: u32,
        position: ChunkPos,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
        procedural: impl FnOnce() -> Result<Option<Chunk>, NativeError>,
    ) -> Result<Option<Chunk>, NativeError> {
        resolve_base(
            db(self.database.begin_read())?,
            dimension,
            position,
            blocks,
            items,
            chest_loot,
            procedural,
        )
    }

    pub fn record_counts(&self) -> Result<NativeRecordCounts, NativeError> {
        count_records(&db(self.database.begin_read())?)
    }
}

fn resolve_base(
    transaction: redb::ReadTransaction,
    dimension: u32,
    position: ChunkPos,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    chest_loot: &ChestLootCatalog,
    procedural: impl FnOnce() -> Result<Option<Chunk>, NativeError>,
) -> Result<Option<Chunk>, NativeError> {
    let geometry = checkpoint::read_geometry(&transaction, dimension)?;
    let imported = db(transaction.open_table(IMPORTED))?;
    let key = chunk_key(dimension, position);
    if let Some(record) = db(imported.get(key.as_slice()))? {
        let root = codec::decode(record.value())?;
        let mut chunk = chunk_from_nbt_with_items_at_position_at_tick(
            &root,
            position,
            blocks,
            Some(items),
            chest_loot,
            geometry,
            0,
        )?;
        ticks::restore(&root, &mut chunk, blocks)?;
        check_geometry(&chunk)?;
        return Ok(Some(chunk));
    }
    drop(imported);
    drop(transaction);
    let chunk = procedural()?;
    if let Some(chunk) = &chunk {
        check_geometry(chunk)?;
        if chunk.geometry() != geometry {
            return Err(NativeError::Invalid("procedural base geometry mismatch"));
        }
        if chunk.pos != position {
            return Err(NativeError::Invalid("procedural base position mismatch"));
        }
    }
    Ok(chunk)
}

impl NativeStore {
    #[must_use]
    pub fn identity(&self) -> [u8; 32] {
        self.identity
    }

    /// Creates only a new database; the owning prepare operation publishes the
    /// checked identity and state-store manifest after durable creation.
    pub fn create(path: &Path, identity: [u8; 32]) -> Result<Self, NativeError> {
        Self::create_with_cache(path, identity, DB_CACHE_BYTES)
    }

    pub fn create_with_cache(
        path: &Path,
        identity: [u8; 32],
        cache_bytes: usize,
    ) -> Result<Self, NativeError> {
        if cache_bytes == 0 {
            return Err(NativeError::Invalid("database cache must be positive"));
        }
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(path)?;
        let database = db(Database::builder()
            .set_cache_size(cache_bytes)
            .create_file(file))?;
        let mut transaction = db(database.begin_write())?;
        db(transaction.set_durability(Durability::Immediate))?;
        {
            let mut header = db(transaction.open_table(HEADER))?;
            db(header.insert("identity", identity.as_slice()))?;
            db(header.insert("schema", NATIVE_PROTOTYPE_SCHEMA.to_be_bytes().as_slice()))?;
            drop(header);
            db(transaction.open_table(IMPORTED))?;
            db(transaction.open_table(OVERLAYS))?;
            db(transaction.open_table(METADATA))?;
            db(transaction.open_table(APPLIED))?;
            db(transaction.open_table(GENERATED))?;
            dictionary::initialize(&transaction)?;
            state::initialize(&transaction)?;
            checkpoint::initialize(&transaction)?;
        }
        db(transaction.commit())?;
        crate::atomic_file::sync_parent_dir(path)?;
        Ok(Self { database, identity })
    }

    /// Does not create a missing database or accept a different base identity.
    pub fn open(path: &Path, identity: [u8; 32]) -> Result<Self, NativeError> {
        Self::open_with_cache(path, identity, DB_CACHE_BYTES)
    }

    pub fn open_with_cache(
        path: &Path,
        identity: [u8; 32],
        cache_bytes: usize,
    ) -> Result<Self, NativeError> {
        if cache_bytes == 0 {
            return Err(NativeError::Invalid("database cache must be positive"));
        }
        let database = db(Database::builder().set_cache_size(cache_bytes).open(path))?;
        {
            let transaction = db(database.begin_read())?;
            validate_header(&transaction, identity)?;
        }
        Ok(Self { database, identity })
    }

    /// Offline inspection never opens the database for writing or repairs it.
    pub fn inspect_read_only(
        path: &Path,
        identity: [u8; 32],
        cache_bytes: usize,
    ) -> Result<NativeRecordCounts, NativeError> {
        if cache_bytes == 0 {
            return Err(NativeError::Invalid("database cache must be positive"));
        }
        let database = read_only_database(path, cache_bytes)?;
        let transaction = db(database.begin_read())?;
        validate_header(&transaction, identity)?;
        count_records(&transaction)
    }

    /// Imported full chunks, including all-air chunks, are canonical durable
    /// baseline records, not overlay/cache entries. Never overwrites coverage.
    /// W0 accepts normalized Solaris compact chunks, not arbitrary Java saves.
    pub fn import_chunk(
        &self,
        dimension: u32,
        chunk: &Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<(), NativeError> {
        self.import_chunk_with_durability(dimension, chunk, blocks, items, Durability::Immediate)
    }

    /// Write a baseline only into an unpublished offline-conversion staging world.
    /// These transactions deliberately do not fsync individually. The converter
    /// must complete its Immediate `commit_checkpoint` before publishing the world;
    /// an interrupted staging world must never be admitted as a live world.
    pub fn import_staged_chunk(
        &self,
        dimension: u32,
        chunk: &Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<(), NativeError> {
        self.import_chunk_with_durability(dimension, chunk, blocks, items, Durability::None)
    }

    fn import_chunk_with_durability(
        &self,
        dimension: u32,
        chunk: &Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        durability: Durability,
    ) -> Result<(), NativeError> {
        check_geometry(chunk)?;
        if chunk.world_journal_lsn() != 0 {
            return Err(NativeError::Invalid(
                "imported baseline carries a native journal LSN",
            ));
        }
        let bytes = codec::baseline(chunk, blocks, items)?;
        let key = chunk_key(dimension, chunk.pos);
        let mut transaction = db(self.database.begin_write())?;
        db(transaction.set_durability(durability))?;
        checkpoint::check_write_geometry(&transaction, dimension, chunk)?;
        {
            let mut imported = db(transaction.open_table(IMPORTED))?;
            if db(imported.get(key.as_slice()))?.is_some() {
                return Err(NativeError::Invalid("imported chunk already exists"));
            }
            // A source cannot be switched after gameplay state has been saved.
            let applied = db(transaction.open_table(APPLIED))?;
            if db(applied.get(key.as_slice()))?.is_some() {
                return Err(NativeError::Invalid("cannot import over gameplay state"));
            }
            db(imported.insert(key.as_slice(), bytes.as_slice()))?;
        }
        db(transaction.commit())
    }

    /// Imported presence wins for the entire compact chunk. The explicit
    /// procedural closure is not evaluated for imported coverage, even air.
    /// The caller supplies the immutable missing-chunk policy; None means deny.
    pub fn resolve_base(
        &self,
        dimension: u32,
        position: ChunkPos,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
        procedural: impl FnOnce() -> Result<Option<Chunk>, NativeError>,
    ) -> Result<Option<Chunk>, NativeError> {
        resolve_base(
            db(self.database.begin_read())?,
            dimension,
            position,
            blocks,
            items,
            chest_loot,
            procedural,
        )
    }

    /// Atomically applies already-journal-durable after-images. Does not clear
    /// resident dirty state or truncate any journal. Replayed older/equal LSNs
    /// cannot overwrite newer snapshots. A clean read writes no record.
    pub fn apply_snapshots(
        &self,
        snapshots: &[NativeChunkSnapshot<'_>],
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<usize, NativeError> {
        self.apply_batch(snapshots, &[], blocks, items)
    }

    /// All participants (including entity migrations and consumed slots) commit
    /// in one immediate transaction. Call only after the owner journal barrier.
    pub fn apply_batch(
        &self,
        snapshots: &[NativeChunkSnapshot<'_>],
        entities: &[NativeEntitySnapshot],
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<usize, NativeError> {
        self.commit_batch(snapshots, entities, blocks, items, SnapshotCommit::Replay)
    }

    fn commit_batch(
        &self,
        snapshots: &[NativeChunkSnapshot<'_>],
        entities: &[NativeEntitySnapshot],
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        mode: SnapshotCommit,
    ) -> Result<usize, NativeError> {
        let mut transaction = db(self.database.begin_write())?;
        db(transaction.set_durability(Durability::Immediate))?;
        let floor = checkpoint::checkpoint_floor(&transaction)?;
        let written = write_snapshots(&transaction, snapshots, blocks, items, mode, floor, None)?;
        let entities_written = state::write(&transaction, entities, floor)?;
        if written != 0 || entities_written != 0 {
            db(transaction.commit())?;
        }
        Ok(written + entities_written)
    }

    /// Offline lease owner mutation. No runtime journal is active: the immediate
    /// DB transaction is the commit authority, and its LSN increments atomically.
    pub fn apply_offline_snapshot(
        &self,
        dimension: u32,
        base: &Chunk,
        effective: &mut Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
    ) -> Result<u64, NativeError> {
        let mut transaction = db(self.database.begin_write())?;
        db(transaction.set_durability(Durability::Immediate))?;
        let floor = checkpoint::checkpoint_floor(&transaction)?;
        let key = chunk_key(dimension, base.pos);
        let previous = {
            let applied = db(transaction.open_table(APPLIED))?;
            db(applied.get(key.as_slice()))?.map_or(0, |lsn| lsn.value())
        };
        let next = previous
            .max(floor.unwrap_or(0))
            .checked_add(1)
            .ok_or(NativeError::Invalid("native LSN exhausted"))?;
        effective.set_world_journal_lsn(next);
        write_snapshots(
            &transaction,
            &[NativeChunkSnapshot {
                dimension,
                base,
                effective,
                dirty_sections: None,
            }],
            blocks,
            items,
            SnapshotCommit::Replay,
            floor,
            None,
        )?;
        db(transaction.commit())?;
        Ok(next)
    }

    /// Read-only composition never activates generated content.
    pub fn compose(
        &self,
        dimension: u32,
        base: &Chunk,
        blocks: &BlockRegistry,
        items: &ItemRegistry,
        chest_loot: &ChestLootCatalog,
    ) -> Result<Chunk, NativeError> {
        compose(
            &db(self.database.begin_read())?,
            dimension,
            base,
            blocks,
            items,
            chest_loot,
        )
    }

    pub fn block_origin(
        &self,
        dimension: u32,
        position: crate::BlockPos,
    ) -> Result<NativeBlockOrigin, NativeError> {
        block_origin(&db(self.database.begin_read())?, dimension, position)
    }
    /// Logical stored key/value bytes across all normal tables (no page overhead).
    /// Read-only diagnostic for checkpoint write-stability work; never a quota.
    pub fn logical_data_bytes(&self) -> Result<u64, NativeError> {
        use redb::ReadableTableMetadata;
        let transaction = db(self.database.begin_read())?;
        let mut total = 0u64;
        for handle in db(transaction.list_tables())? {
            let table = db(transaction.open_untyped_table(handle))?;
            total = total.saturating_add(db(table.stats())?.stored_bytes());
        }
        Ok(total)
    }

    pub fn record_counts(&self) -> Result<NativeRecordCounts, NativeError> {
        count_records(&db(self.database.begin_read())?)
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SnapshotCommit {
    Replay,
    WorldCheckpoint(u64),
}

fn write_snapshots(
    transaction: &redb::WriteTransaction,
    snapshots: &[NativeChunkSnapshot<'_>],
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    mode: SnapshotCommit,
    floor: Option<u64>,
    mut io: Option<&mut NativeCheckpointIo>,
) -> Result<usize, NativeError> {
    let mut written = 0;
    let mut overlays = db(transaction.open_table(OVERLAYS))?;
    let mut metadata = db(transaction.open_table(METADATA))?;
    let mut applied = db(transaction.open_table(APPLIED))?;
    let mut dictionary = dictionary::Writer::open(transaction)?;
    let mut seen = std::collections::BTreeSet::new();
    for snapshot in snapshots {
        checkpoint::check_write_geometry(transaction, snapshot.dimension, snapshot.base)?;
        check_geometry(snapshot.effective)?;
        if snapshot.base.geometry() != snapshot.effective.geometry() {
            return Err(NativeError::Invalid("snapshot baseline geometry mismatch"));
        }
        if snapshot.base.pos != snapshot.effective.pos {
            return Err(NativeError::Invalid("snapshot baseline position mismatch"));
        }
        let position = snapshot.base.pos;
        let key = chunk_key(snapshot.dimension, position);
        if !seen.insert(key) {
            return Err(NativeError::Invalid("duplicate snapshot participant"));
        }
        // The mask narrows only world checkpoints: replay probes every stored
        // section record to decide `changed`, so it keeps examining all of them.
        let dirty = match mode {
            SnapshotCommit::WorldCheckpoint(_) => checked_dirty_sections(snapshot)?,
            SnapshotCommit::Replay => None,
        };
        let selected = |index: usize| dirty.is_none_or(|mask| (mask >> index) & 1 == 1);
        // Overlay evaluations of the unchanged-capture probe, reused below.
        let mut staged: Vec<(usize, Option<Vec<u8>>)> = Vec::new();
        let lsn = match mode {
            SnapshotCommit::WorldCheckpoint(tick) => tick,
            _ => snapshot.effective.world_journal_lsn(),
        };
        if let Some(previous) = db(applied.get(key.as_slice()))? {
            if lsn == 0 && mode == SnapshotCommit::Replay {
                return Err(NativeError::Invalid(
                    "unstamped snapshot over durable state",
                ));
            }
            if previous.value() >= lsn && mode == SnapshotCommit::Replay {
                continue;
            }
            if previous.value() > lsn {
                return Err(NativeError::Invalid(
                    "resident snapshot precedes durable state",
                ));
            }
        }
        // The admitted checkpoint covers all older journal decisions, even
        // when unchanged rows retained an earlier per-record stamp.
        if mode == SnapshotCommit::Replay && lsn != 0 && floor.is_some_and(|tick| lsn <= tick) {
            continue;
        }
        // World checkpoints retain stored bytes (and the prior APPLIED LSN)
        // when the capture is unchanged at a later global tick. Comparison is
        // semantic on chunk content, ignoring only the capture stamp;
        // a real block or side-state delta always falls through and stamps
        // the new tick below. Replay keeps its existing LSN fences untouched.
        if let SnapshotCommit::WorldCheckpoint(tick) = mode
            && let Some(prior) = db(applied.get(key.as_slice()))?.map(|record| record.value())
            && prior <= tick
        {
            // Compare recomputed bytes at the prior stamp: the journal stamp is
            // capture metadata, not chunk data. metadata_at_lsn keeps the same
            // field order as the write path, so equal content means equal bytes.
            let expected = codec::metadata_at_lsn(snapshot.effective, blocks, items, prior)?;
            let stored = db(metadata.get(key.as_slice()))?;
            let meta_equal = match &stored {
                // Matches the write path: equal base/effective means no row.
                None => expected == codec::metadata(snapshot.base, blocks, items)?,
                Some(record) => record.value() == expected.as_slice(),
            };
            drop(stored);
            if meta_equal {
                let mut unchanged = true;
                for (index, (base_section, effective_section)) in snapshot
                    .base
                    .sections
                    .iter()
                    .zip(&snapshot.effective.sections)
                    .enumerate()
                    .filter(|(index, _)| selected(*index))
                {
                    let section_y = snapshot.base.geometry().min_y() / 16 + index as i32;
                    let section_key = section_key(snapshot.dimension, position, section_y);
                    let previous = db(overlays.get(section_key.as_slice()))?;
                    let bytes = codec::overlay(
                        base_section,
                        effective_section,
                        blocks,
                        &mut dictionary,
                        previous.as_ref().map(|record| record.value()),
                    )?;
                    if let Some(io) = io.as_deref_mut() {
                        io.sections_encoded += 1;
                    }
                    let same = bytes.as_deref() == previous.as_ref().map(|record| record.value());
                    drop(previous);
                    staged.push((index, bytes));
                    if !same {
                        unchanged = false;
                        break;
                    }
                }
                if unchanged {
                    continue;
                }
            }
        }
        let base_metadata = codec::metadata(snapshot.base, blocks, items)?;
        let effective_metadata = match mode {
            SnapshotCommit::WorldCheckpoint(tick) => {
                codec::metadata_at_lsn(snapshot.effective, blocks, items, tick)?
            }
            _ => codec::metadata(snapshot.effective, blocks, items)?,
        };
        let mut changed = base_metadata != effective_metadata
            || matches!(mode, SnapshotCommit::WorldCheckpoint(_));
        changed |= db(metadata.get(key.as_slice()))?.is_some();
        let mut staged = staged.into_iter().peekable();
        for (index, (base, effective)) in snapshot
            .base
            .sections
            .iter()
            .zip(&snapshot.effective.sections)
            .enumerate()
            .filter(|(index, _)| selected(*index))
        {
            let section_y = snapshot.base.geometry().min_y() / 16 + index as i32;
            let section_key = section_key(snapshot.dimension, position, section_y);
            let previous = db(overlays.get(section_key.as_slice()))?;
            let bytes = match staged.next_if(|(staged_index, _)| *staged_index == index) {
                Some((_, bytes)) => bytes,
                None => {
                    let bytes = codec::overlay(
                        base,
                        effective,
                        blocks,
                        &mut dictionary,
                        previous.as_ref().map(|record| record.value()),
                    )?;
                    if let Some(io) = io.as_deref_mut() {
                        io.sections_encoded += 1;
                    }
                    bytes
                }
            };
            changed |= previous.is_some();
            drop(previous);
            match bytes {
                Some(bytes) => {
                    changed = true;
                    db(overlays.insert(section_key.as_slice(), bytes.as_slice()))?;
                    if let Some(io) = io.as_deref_mut() {
                        io.section_records_written += 1;
                    }
                    if let Some(io) = io.as_deref_mut() {
                        io.chunk_payload_bytes += bytes.len() as u64;
                    }
                }
                None => {
                    db(overlays.remove(section_key.as_slice()))?;
                }
            }
        }
        if !changed {
            continue;
        }
        if lsn == 0 && mode == SnapshotCommit::Replay {
            return Err(NativeError::Invalid(
                "changed snapshot must have a journal decision",
            ));
        }
        if base_metadata == effective_metadata {
            db(metadata.remove(key.as_slice()))?;
        } else {
            db(metadata.insert(key.as_slice(), effective_metadata.as_slice()))?;
            if let Some(io) = io.as_deref_mut() {
                io.chunk_payload_bytes += effective_metadata.len() as u64;
            }
        }
        db(applied.insert(key.as_slice(), lsn))?;
        written += 1;
        if let Some(io) = io.as_deref_mut() {
            io.rewritten_chunks += 1;
        }
    }
    Ok(written)
}

fn checked_dirty_sections(snapshot: &NativeChunkSnapshot<'_>) -> Result<Option<u32>, NativeError> {
    match snapshot.dirty_sections {
        Some(mask) if u64::from(mask) >> snapshot.effective.sections.len() != 0 => {
            Err(NativeError::Invalid("dirty section outside chunk geometry"))
        }
        mask => Ok(mask),
    }
}

fn compose(
    transaction: &redb::ReadTransaction,
    dimension: u32,
    base: &Chunk,
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    chest_loot: &ChestLootCatalog,
) -> Result<Chunk, NativeError> {
    checkpoint::check_read_geometry(transaction, dimension, base)?;
    let dictionary = dictionary::Reader::open(transaction)?;
    let overlays = db(transaction.open_table(OVERLAYS))?;
    let metadata = db(transaction.open_table(METADATA))?;
    let applied = db(transaction.open_table(APPLIED))?;
    let key = chunk_key(dimension, base.pos);
    let state = db(metadata.get(key.as_slice()))?;
    let mut root = match &state {
        Some(record) => codec::decode(record.value())?,
        None => codec::metadata_root(base, blocks, items)?,
    };
    let sections = codec::sections_mut(&mut root)?;
    let min_section_y = base.geometry().min_y() / 16;
    let mut seen = vec![false; base.geometry().section_count()];
    for fields in sections {
        let section_y = codec::section_y(fields)?;
        let Some(index) = section_y
            .checked_sub(min_section_y)
            .and_then(|index| usize::try_from(index).ok())
            .filter(|index| *index < base.sections.len())
        else {
            if section_y == min_section_y - 1
                || section_y == min_section_y + base.sections.len() as i32
            {
                continue;
            }
            return Err(NativeError::Invalid("metadata geometry mismatch"));
        };
        if seen[index] {
            return Err(NativeError::Invalid("duplicate metadata section"));
        }
        seen[index] = true;
        let section = &base.sections[index];
        let section_key = section_key(dimension, base.pos, section_y);
        let encoded = match db(overlays.get(section_key.as_slice()))? {
            Some(record) => codec::compose_section(section, record.value(), blocks, &dictionary)?,
            None => crate::anvil::chunk_nbt::encode_block_section(section, blocks)?,
        };
        codec::compound_mut(fields)?.push(("block_states".into(), encoded));
    }
    if seen.iter().any(|present| !present) {
        return Err(NativeError::Invalid("missing metadata section"));
    }
    let mut chunk = chunk_from_nbt_with_items_at_position_at_tick(
        &root,
        base.pos,
        blocks,
        Some(items),
        chest_loot,
        base.geometry(),
        0,
    )?;
    ticks::restore(&root, &mut chunk, blocks)?;
    check_geometry(&chunk)?;
    if let Some(lsn) = db(applied.get(key.as_slice()))?
        && chunk.world_journal_lsn() != lsn.value()
    {
        return Err(NativeError::Invalid("metadata/applied LSN mismatch"));
    }
    chunk.dirty = false;
    Ok(chunk)
}

fn validate_header(
    transaction: &redb::ReadTransaction,
    identity: [u8; 32],
) -> Result<(), NativeError> {
    let header = db(transaction.open_table(HEADER))?;
    let schema = db(header.get("schema"))?.ok_or(NativeError::Invalid("native schema missing"))?;
    let schema = u32::from_be_bytes(
        schema
            .value()
            .try_into()
            .map_err(|_| NativeError::Invalid("invalid native schema"))?,
    );
    if schema != NATIVE_PROTOTYPE_SCHEMA {
        return Err(NativeError::UnsupportedSchema(schema));
    }
    if db(header.get("identity"))?.is_none_or(|value| value.value() != identity) {
        return Err(NativeError::Invalid("identity mismatch"));
    }
    db(transaction.open_table(IMPORTED))?;
    db(transaction.open_table(OVERLAYS))?;
    db(transaction.open_table(METADATA))?;
    db(transaction.open_table(APPLIED))?;
    dictionary::Reader::open(transaction)?;
    db(transaction.open_table(state::ENTITIES))?;
    db(transaction.open_table(state::INITIALIZED))?;
    db(transaction.open_table(state::ENTITY_LSN))?;
    checkpoint::validate_tables(transaction)?;
    Ok(())
}

fn count_records(transaction: &redb::ReadTransaction) -> Result<NativeRecordCounts, NativeError> {
    Ok(NativeRecordCounts {
        imported_chunks: db(db(transaction.open_table(IMPORTED))?.len())?,
        overlay_sections: db(db(transaction.open_table(OVERLAYS))?.len())?,
        metadata_chunks: db(db(transaction.open_table(METADATA))?.len())?,
        applied_chunks: db(db(transaction.open_table(APPLIED))?.len())?,
        persistent_entities: db(db(transaction.open_table(state::ENTITIES))?.len())?,
        initialized_slots: db(db(transaction.open_table(state::INITIALIZED))?.len())?,
        stable_states: db(db(transaction.open_table(dictionary::STATES))?.len())?,
    })
}

fn check_geometry(chunk: &Chunk) -> Result<(), NativeError> {
    if chunk.sections.len() != chunk.geometry().section_count() {
        return Err(NativeError::Invalid(
            "native chunk section geometry mismatch",
        ));
    }
    Ok(())
}

// Big-endian coordinates with the sign bit flipped: lexicographic order is
// signed coordinate order. Dimension is an explicit unsigned namespace.
fn chunk_key(dimension: u32, position: ChunkPos) -> [u8; 12] {
    let mut key = [0; 12];
    key[..4].copy_from_slice(&dimension.to_be_bytes());
    key[4..8].copy_from_slice(&((position.x as u32) ^ 0x8000_0000).to_be_bytes());
    key[8..].copy_from_slice(&((position.z as u32) ^ 0x8000_0000).to_be_bytes());
    key
}

fn section_key(dimension: u32, position: ChunkPos, section_y: i32) -> [u8; 16] {
    let mut key = [0; 16];
    key[..12].copy_from_slice(&chunk_key(dimension, position));
    key[12..].copy_from_slice(&((section_y as u32) ^ 0x8000_0000).to_be_bytes());
    key
}
