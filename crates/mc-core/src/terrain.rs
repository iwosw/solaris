//! Bounded immutable real chunk terrain sampling for the optional DH LOD presentation.
//!
//! A terrain section is a detail-6 DH leaf — 4x4 chunks of real block/biome/light
//! data — plus a one-chunk halo on every side (6x6 total), so the presentation
//! layer can apply the same visible-material policy as vanilla packets without
//! a second store lookup. Live committed resident chunks outrank the durable
//! native store; missing chunks resolve through the normal load-or-generate
//! path with light baked on workers ([`crate::chunk_loader::ChunkSource`]).
//! Corrupt stored coverage fails closed: it is never replaced with generated
//! terrain. No simulation residency, no native writes, no networking/DTO types.
//!
//! Publication freshness has two levels. Each [`TerrainSection`] carries its
//! own folded `revision` over all 36 chunks plus a content-pinned `modified_ms`
//! timestamp (never wall-clock-now per encode; process-local pinned and
//! strictly increasing, never stale). Stamps are not restart-stable: after a
//! restart or stamp-map eviction a re-sampled revision gets a fresh wall-clock
//! stamp (conservative spurious refresh).
//! [`Core::terrain_revision`] is the cheap per-dimension global fence: the
//! publication tick of the last terrain-changing publication (block writes,
//! admissions, evictions); idle ticks leave it untouched so slow generations
//! are not rejected and caches do not churn. It returns `u64::MAX` while a
//! terrain-changing publication swaps its coherent bundle (busy) or for an
//! unknown dimension; samplers reject busy or changed fences and retry.
//! The authoritative position/dimension lifecycle still rides `TickOutput`.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use mc_world::{Chunk, ChunkPos};

use crate::CoreError;

/// Requested leaf width in chunks (DH detail 6 = 64 blocks = 4 chunks).
pub const TERRAIN_LEAF: usize = 4;
/// Halo width in chunks on each side for the visible-material policy.
pub const TERRAIN_HALO: usize = 1;
/// Sampled width in chunks: leaf plus halo on both sides.
pub const TERRAIN_WIDTH: usize = TERRAIN_LEAF + 2 * TERRAIN_HALO;
/// Bounded chunks per call. Conservative and fixed: no coordinate can widen it.
pub const TERRAIN_CHUNKS: usize = TERRAIN_WIDTH * TERRAIN_WIDTH;

/// Immutable sampled section: the requested 4x4 leaf centered in a 6x6 chunk
/// grid. Row-major `chunks[dz * TERRAIN_WIDTH + dx]`, `dx/dz in 0..6`; the
/// core leaf is `dx/dz in 1..5`. The presentation layer selects the leaf
/// (offset by [`TERRAIN_HALO`]) for encoding and reads the halo only for the
/// edge policy.
#[derive(Debug, Clone)]
pub struct TerrainSection {
    /// Native dimension id (`0` overworld, `1` nether, `2` end).
    pub dimension: u32,
    /// DH detail-6 leaf coordinates: `floor(block / 64)`.
    pub section_x: i32,
    pub section_z: i32,
    /// Exactly [`TERRAIN_CHUNKS`] immutable chunks in row-major order.
    pub chunks: Vec<Arc<Chunk>>,
    /// Folded content revision over all 36 chunks (resident wire revisions,
    /// stable identity for generated/native images). Any of the 36 changing
    /// yields a different value.
    pub revision: u64,
    /// Unix milliseconds pinned at the revision change: the same revision
    /// carries the same timestamp across calls within one process run, so
    /// timestamp-sync requests compare correctly without re-encoding.
    /// Strictly increasing across revisions (see `stamp`). NOT stable
    /// across restarts or stamp-map evictions: a re-sampled revision gets a
    /// fresh wall-clock stamp (conservative spurious refresh, never stale).
    /// Never wall-clock-now per encode.
    pub modified_ms: i64,
    /// Opaque stable world epoch: random per world, persisted through the
    /// native owner path, preserved across restarts, rotated on world reset.
    /// Never derived from the seed.
    pub epoch: String,
}

/// Committed-but-unflushed terrain writes published by the tick owner for
/// off-tick sampling: resident dirty chunks (edits not yet checkpoint-flushed;
/// eviction retains them) plus non-resident overlay writes (retained across
/// fold until checkpoint materialize). Covers the edit -> evict -> sample
/// before checkpoint window, when neither the latest snapshot nor the native
/// store carries the edit. Empty in steady state (shared `EMPTY`).
#[derive(Debug, Default)]
pub(crate) struct DirtySupplement {
    /// Resident dirty chunks: position, wire revision, immutable chunk.
    pub chunks: Vec<(ChunkPos, u64, Arc<Chunk>)>,
    /// Non-resident overlay writes: block position, state id.
    pub overlay: Vec<((i32, i32, i32), u32)>,
}

impl DirtySupplement {
    pub(crate) fn empty() -> Arc<Self> {
        static EMPTY: std::sync::LazyLock<Arc<DirtySupplement>> =
            std::sync::LazyLock::new(|| Arc::new(DirtySupplement::default()));
        Arc::clone(&EMPTY)
    }
}

/// Off-tick immutable terrain source for one dimension: the authoritative
/// store/generator resolver plus the world's stable epoch and a cheap global
/// publication fence. Shares the native store read-only access with the tick
/// workers; the returned chunks are owned `Arc`s, never borrowed tick state.
pub(crate) struct TerrainSource {
    source: std::sync::Weak<crate::chunk_loader::ChunkSource>,
    /// Coherent published bundle: the committed resident snapshot and the
    /// committed-but-unflushed supplement published under the fence value
    /// mirrored in `publication`. Both swap atomically under one write
    /// lock, so off-tick sampling never mixes a new snapshot with an old
    /// supplement (or the reverse) under an unchanged fence. The fence
    /// itself lives only in the atomic below: idle ticks touch neither, so
    /// bundle and fence stay stable together with nothing to drift.
    bundle: parking_lot::RwLock<PublishedTerrain>,
    /// Cheap atomic mirror of the bundle's fence value (one atomic load, no
    /// locks). The only writer is the tick owner inside the coherent swap;
    /// readers reject `u64::MAX` (busy) before sampling.
    publication: AtomicU64,
    /// Timestamps pinned per section revision: revision -> modified ms,
    /// bounded (spurious eviction only causes a conservative re-stamp).
    stamps: parking_lot::Mutex<std::collections::HashMap<u64, i64>>,
    /// Stable opaque world identity (random hex, not seed-derived).
    epoch: String,
}

/// One coherent terrain publication. Readers hold the lock only while cloning
/// its `Arc`s, never during store reads or generation. The tick owner never
/// mutates a published chunk. Idle ticks leave the bundle and fence untouched.
#[derive(Debug, Clone, Default)]
struct PublishedTerrain {
    /// Committed resident snapshot (owned `Arc`, no tick-state borrow).
    snapshot: Option<Arc<crate::boundary::WorldSnapshot>>,
    /// Committed-but-unflushed supplement at the same publication point.
    supplement: Arc<DirtySupplement>,
}

impl std::fmt::Debug for TerrainSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerrainSource")
            .field(
                "dimension",
                &self.source.upgrade().map(|source| source.dimension),
            )
            .finish_non_exhaustive()
    }
}

impl TerrainSource {
    pub(crate) fn new(source: Arc<crate::chunk_loader::ChunkSource>, epoch: String) -> Self {
        Self {
            source: Arc::downgrade(&source),
            bundle: parking_lot::RwLock::new(PublishedTerrain::default()),
            publication: AtomicU64::new(0),
            stamps: parking_lot::Mutex::new(std::collections::HashMap::new()),
            epoch,
        }
    }

    /// Cheap global fence for this dimension's cache invalidation (atomic
    /// load, no locks). Advanced by the tick owner only when a publication
    /// carries terrain changes (new/resident-changed chunks or block edits);
    /// idle ticks (no chunk delta) leave it untouched so the edge
    /// before/after sampling check does not reject slow generations and the
    /// shared cache does not invalidate continuously. `u64::MAX` means busy:
    /// a terrain-changing publication is swapping its coherent bundle right
    /// now; the sampler must reject and retry instead of mixing data.
    pub(crate) fn revision(&self) -> u64 {
        self.publication.load(Ordering::Acquire)
    }

    /// Mark the fence busy while the coherent bundle swaps. Called by the
    /// tick owner before touching any per-dimension bundle of a
    /// terrain-changing publication (`u64::MAX` is never a real fence).
    pub(crate) fn begin_publish(&self) {
        self.publication.store(u64::MAX, Ordering::Release);
    }

    /// Atomically swap one dimension's coherent (snapshot, supplement)
    /// bundle and mirror the fence. Called by the tick owner while busy;
    /// readers holding the read guard see either the fully old or the
    /// fully new bundle, never a mixed pair.
    pub(crate) fn finish_publish(
        &self,
        snapshot: Option<Arc<crate::boundary::WorldSnapshot>>,
        supplement: Arc<DirtySupplement>,
        revision: u64,
    ) {
        debug_assert_ne!(
            revision,
            u64::MAX,
            "MAX is the busy fence, never a real tick"
        );
        *self.bundle.write() = PublishedTerrain {
            snapshot,
            supplement,
        };
        self.publication.store(revision, Ordering::Release);
    }

    /// Test-only supplement injection, preserving the published snapshot and
    /// fence. Gated on `cfg(test)` so production code cannot bypass the
    /// coherent path: the tick owner must use
    /// [`TerrainSource::begin_publish`]/[`TerrainSource::finish_publish`]
    /// so snapshot, supplement and fence swap together.
    #[cfg(test)]
    pub(crate) fn publish_supplement(&self, supplement: Arc<DirtySupplement>) {
        self.bundle.write().supplement = supplement;
    }

    #[cfg(test)]
    pub(crate) fn snapshot(&self) -> Option<Arc<crate::boundary::WorldSnapshot>> {
        self.bundle.read().snapshot.clone()
    }

    /// Unix milliseconds pinned to a section revision. The same revision
    /// always maps to the same stamp within this process. A new revision
    /// receives a stamp strictly greater than every stamp allocated before
    /// it (`max + 1` lower bound), so two edits landing in the same
    /// wall-clock millisecond still order correctly and a newer revision
    /// never satisfies an older timestamp. Stamps come from the wall clock
    /// lower-bounded by that strict bound, so allocation order is monotonic
    /// even across clock steps. Bounded map: evicting an old revision
    /// re-stamps it from the wall clock on next sight (conservative
    /// spurious refresh, never stale). NOT restart-stable by itself: after
    /// a restart the map is empty, so a re-sampled unchanged revision gets
    /// a fresh wall-clock stamp. Edge caches keyed on
    /// `(epoch, dimension, section, revision)` are still correct across
    /// restarts (same revision = same bytes, modulo the timestamp field
    /// itself); callers needing a persistent revision->timestamp mapping
    /// must durably record the stamp the first time a revision is encoded.
    fn stamp(&self, revision: u64) -> i64 {
        const STAMP_CAP: usize = 1024;
        let mut stamps = self.stamps.lock();
        if let Some(&modified) = stamps.get(&revision) {
            return modified;
        }
        let now = unix_ms();
        let max = stamps.values().copied().max().unwrap_or(0);
        let modified = now.max(max.saturating_add(1)).max(1);
        if stamps.len() >= STAMP_CAP {
            // Evict the oldest stamp by value (arbitrary but bounded); a
            // re-stamp is conservative, never stale.
            if let Some(oldest) = stamps
                .iter()
                .min_by_key(|&(_, &ms)| ms)
                .map(|(&rev, _)| rev)
            {
                stamps.remove(&oldest);
            }
        }
        stamps.insert(revision, modified);
        modified
    }

    /// Committed-but-unflushed supplement published by the tick owner:
    /// resident dirty chunks plus non-resident overlay writes. Covers the
    /// edit-then-evict-then-sample-before-checkpoint window: eviction
    /// retains dirty chunks, and non-resident overlay writes are retained
    /// across fold, so both survive here until the checkpoint ACK clears
    /// them. Usually shared-empty (no per-tick allocation when clean).
    /// Read from the coherent bundle; the returned `Arc` is the exact
    /// supplement published with the sampled snapshot.
    #[cfg(test)]
    pub(crate) fn supplement(&self) -> Arc<DirtySupplement> {
        Arc::clone(&self.bundle.read().supplement)
    }

    pub(crate) fn empty_supplement() -> Arc<DirtySupplement> {
        DirtySupplement::empty()
    }

    /// Sample one 6x6 section off the tick thread from the latest published
    /// coherent bundle (resident snapshot plus committed-but-unflushed
    /// supplement swapped together), then the durable native store as the
    /// fallback for non-resident chunks.
    /// Precedence: published resident chunk, then supplement dirty chunk,
    /// then supplement overlay writes applied over the base, then the
    /// store's native image, then generation for genuinely missing
    /// coverage. Corrupt stored coverage fails closed (never router/air
    /// substitution). No simulation residency, no native writes, no
    /// tick-state borrows. Store reads and generation hold no publication
    /// lock and run on the caller's bounded LOD worker thread.
    /// The caller must hold a fence value from [`TerrainSource::revision`]
    /// across the call and reject `u64::MAX` (busy) before sampling and a
    /// changed fence after; while busy the bundle guard is still coherent,
    /// but the fence contract forces a retry so the edge never caches a
    /// section under a fence that already moved.
    pub(crate) fn section(
        &self,
        dimension: u32,
        section_x: i32,
        section_z: i32,
    ) -> Result<TerrainSection, CoreError> {
        // Fails closed on unknown dimensions (0/1/2 only); `dimension_of`
        // maps native id -> closed `Dimension` so it doubles as validator.
        let _ = dimension_of(dimension)?;
        let source = self
            .source
            .upgrade()
            .ok_or_else(|| CoreError::Storage("terrain source stopped".into()))?;
        if dimension != source.dimension {
            return Err(CoreError::Storage(format!(
                "terrain section: dimension {dimension} does not match this source"
            )));
        }
        let before = self.revision();
        if before == u64::MAX {
            return Err(CoreError::Storage(
                "terrain section: publication busy; request again".into(),
            ));
        }
        // Coherent bundle: one read guard covers snapshot + supplement, so
        // the 36 chunks never mix publications. `Arc` clones keep the data
        // alive past the guard; blocking store/generation work below runs
        // without holding it.
        let (resident, supplement) = {
            let bundle = self.bundle.read();
            (bundle.snapshot.clone(), Arc::clone(&bundle.supplement))
        };
        let positions = section_positions(section_x, section_z)?;
        let mut chunks = Vec::with_capacity(TERRAIN_CHUNKS);
        let mut keys = [0; TERRAIN_CHUNKS];
        // Bounded: overlay writes outside this section's 6x6 footprint skip
        // all per-chunk work. `section_positions` already failed closed on
        // overflow, so the footprint bounds below cannot overflow.
        let section_overlay: &[((i32, i32, i32), u32)] = if supplement.overlay.is_empty() {
            &[]
        } else {
            // Checked footprint: chunk bounds derive from the same checked
            // `section_positions` base, so overflow fails closed instead of
            // wrapping. Halo offsets are ±1 chunk: far below `i32` limits
            // once the base itself is checked.
            let (base_x, base_z) = leaf_base(section_x, section_z)?;
            let min_bx = base_x
                .checked_sub(TERRAIN_HALO as i32)
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            let max_bx = base_x
                .checked_add((TERRAIN_WIDTH as i32) - (TERRAIN_HALO as i32) - 1)
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            let min_bz = base_z
                .checked_sub(TERRAIN_HALO as i32)
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            let max_bz = base_z
                .checked_add((TERRAIN_WIDTH as i32) - (TERRAIN_HALO as i32) - 1)
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            let in_section = |x: i32, z: i32| {
                let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
                cx >= min_bx && cx <= max_bx && cz >= min_bz && cz <= max_bz
            };
            if supplement
                .overlay
                .iter()
                .any(|&((x, _, z), _)| in_section(x, z))
            {
                &supplement.overlay
            } else {
                &[]
            }
        };
        for (index, pos) in positions.iter().enumerate() {
            let (mut revision, mut chunk) = match resident.as_ref().and_then(|snapshot| {
                snapshot
                    .chunks
                    .iter()
                    .find(|entry| entry.pos == *pos)
                    .map(|entry| (entry.revision, Arc::clone(&entry.chunk)))
            }) {
                Some((revision, chunk)) => (revision, chunk),
                None => match supplement.chunks.iter().find(|(p, _, _)| p == pos) {
                    Some((_, revision, chunk)) => (*revision, Arc::clone(chunk)),
                    None => {
                        let (mut chunk, generated) = source
                            .load_or_generate(*pos)
                            .map_err(|error| CoreError::Storage(error.to_string()))?;
                        let key = if generated {
                            // Stable generation identity: same seed + worldgen
                            // revision + position always yields the same key, so
                            // unchanged generated terrain keeps a stable section
                            // revision across restarts and eviction cycles.
                            generated_key(&chunk)
                        } else {
                            // Durable native image: fold its journal LSN into the
                            // key so a loaded-but-edited-then-flushed chunk
                            // invalidates even if the resident edit was evicted.
                            native_key(&chunk)
                        };
                        crate::light::bake_missing(&mut chunk, &source.light);
                        (key, Arc::new(chunk))
                    }
                },
            };
            // Non-resident overlay writes in range apply over the resolved
            // base (copy-on-write: shared snapshot/store chunks are never
            // mutated). Each applied write folds into the revision key so
            // the section revision covers the actual sampled content.
            // Bounded: `section_overlay` is empty unless some write lands in
            // this section's footprint; per chunk only in-range entries are
            // copied (at most the cells of one chunk column).
            let mut applied: Vec<((i32, i32, i32), u32)> = Vec::new();
            if !section_overlay.is_empty() {
                // Checked per-chunk block bounds: `pos * 16` cannot overflow
                // for positions `section_positions` produced (chunk range is
                // 4x-checked), but a corrupt supplement position would wrap
                // the range open — fail closed instead.
                let min_x = pos.x.checked_mul(16).ok_or_else(|| {
                    CoreError::Storage("terrain section outside the world".into())
                })?;
                let max_x = min_x.checked_add(15).ok_or_else(|| {
                    CoreError::Storage("terrain section outside the world".into())
                })?;
                let min_z = pos.z.checked_mul(16).ok_or_else(|| {
                    CoreError::Storage("terrain section outside the world".into())
                })?;
                let max_z = min_z.checked_add(15).ok_or_else(|| {
                    CoreError::Storage("terrain section outside the world".into())
                })?;
                for &((x, y, z), state) in section_overlay {
                    if x >= min_x && x <= max_x && z >= min_z && z <= max_z {
                        applied.push(((x, y, z), state));
                    }
                }
            }
            if !applied.is_empty() {
                applied.sort_unstable();
                let chunk = Arc::make_mut(&mut chunk);
                for &((x, y, z), state) in &applied {
                    let geometry = chunk.geometry();
                    if geometry.world_y_to_section(y).is_none() {
                        return Err(CoreError::Storage(
                            "terrain section: pending write outside chunk geometry".into(),
                        ));
                    }
                    chunk.set_block(
                        x.rem_euclid(16) as u8,
                        y,
                        z.rem_euclid(16) as u8,
                        mc_world::BlockStateId(state),
                    );
                }
                crate::light::bake_missing(chunk, &source.light);
                revision = fold_revision(&[revision, fold_pending(&applied)]);
            }
            if chunk.pos != *pos || chunk.geometry() != source.geometry {
                return Err(CoreError::Storage(
                    "terrain section: produced chunk position or geometry mismatch".into(),
                ));
            }
            keys[index] = revision;
            chunks.push(chunk);
        }
        // Fence moved while sampling: content may mix store reads from both
        // sides of the publication. Reject; the edge retries under the new
        // fence (idle ticks leave the fence untouched, so slow generations
        // are not spuriously rejected).
        let after = self.revision();
        if after == u64::MAX || after != before {
            return Err(CoreError::Storage(
                "terrain section: terrain changed during sampling; request again".into(),
            ));
        }
        let revision = fold_revision(&keys);
        let modified_ms = self.stamp(revision);
        Ok(TerrainSection {
            dimension,
            section_x,
            section_z,
            chunks,
            revision,
            modified_ms,
            epoch: self.epoch.clone(),
        })
    }
}

/// Resolve a closed dimension from its native id.
pub(crate) fn dimension_of(dimension: u32) -> Result<crate::dimensions::Dimension, CoreError> {
    match dimension {
        0 => Ok(crate::dimensions::Dimension::Overworld),
        1 => Ok(crate::dimensions::Dimension::Nether),
        2 => Ok(crate::dimensions::Dimension::End),
        other => Err(CoreError::Storage(format!(
            "terrain section: unknown dimension {other}"
        ))),
    }
}

/// Leaf base chunk from DH detail-6 section coordinates with checked math.
pub(crate) fn leaf_base(section_x: i32, section_z: i32) -> Result<(i32, i32), CoreError> {
    let base_x = section_x
        .checked_mul(TERRAIN_LEAF as i32)
        .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
    let base_z = section_z
        .checked_mul(TERRAIN_LEAF as i32)
        .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
    Ok((base_x, base_z))
}

/// All 36 chunk positions for a section, row-major with the leaf at offset
/// [`TERRAIN_HALO`].
pub(crate) fn section_positions(
    section_x: i32,
    section_z: i32,
) -> Result<[ChunkPos; TERRAIN_CHUNKS], CoreError> {
    let (base_x, base_z) = leaf_base(section_x, section_z)?;
    let mut positions = [ChunkPos { x: 0, z: 0 }; TERRAIN_CHUNKS];
    for dz in 0..TERRAIN_WIDTH {
        for dx in 0..TERRAIN_WIDTH {
            let x = (base_x as i64)
                .checked_add(dx as i64 - TERRAIN_HALO as i64)
                .and_then(|x| i32::try_from(x).ok())
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            let z = (base_z as i64)
                .checked_add(dz as i64 - TERRAIN_HALO as i64)
                .and_then(|z| i32::try_from(z).ok())
                .ok_or_else(|| CoreError::Storage("terrain section outside the world".into()))?;
            positions[dz * TERRAIN_WIDTH + dx] = ChunkPos { x, z };
        }
    }
    Ok(positions)
}

/// Fold per-chunk revision keys into one section revision. Order is fixed
/// row-major, so equal content at equal positions yields equal revisions.
pub(crate) fn fold_revision(keys: &[u64]) -> u64 {
    let mut revision: u64 = 0x9E37_79B9_7F4A_7C15;
    for &key in keys {
        revision = revision
            .wrapping_add(0x9E37_79B9_7F4A_7C15)
            .wrapping_add(key)
            .wrapping_add(revision << 6)
            .wrapping_add(revision >> 2);
    }
    revision
}

/// Stable identity for a generated (never stored) chunk: position plus the
/// immutable worldgen contract (seed is never disclosed — this is a one-way
/// folded key, not the seed). Same generator output at the same position
/// always yields the same key.
fn generated_key(chunk: &Chunk) -> u64 {
    let mut key: u64 = 0xC2B2_AE35_27D4_E395;
    key ^= chunk.pos.x as u32 as u64 | ((chunk.pos.z as u32 as u64) << 32);
    key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    key ^= chunk.geometry().min_y() as u64 | ((chunk.geometry().max_y() as u64) << 32);
    key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    // Fold a cheap content sample: top non-air Y per column corner + centre.
    // Full-content hashing would scan 98k blocks per chunk; the generator is
    // deterministic per position, so position + geometry + a content probe
    // distinguishes generated revisions without a full scan.
    for &(lx, lz) in &[(0u8, 0u8), (15, 0), (0, 15), (15, 15), (8, 8)] {
        let top = (chunk.geometry().min_y()..chunk.geometry().max_y())
            .rev()
            .find(|&y| chunk.get_block(lx, y, lz).is_some_and(|state| state.0 != 0))
            .unwrap_or(chunk.geometry().min_y() - 1);
        key ^= top as u64;
        key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    key
}

/// Fold applied pending overlay writes into a revision key. Sorted by
/// (pos, state) at the call site, so equal write sets yield equal keys.
fn fold_pending(applied: &[((i32, i32, i32), u32)]) -> u64 {
    let mut key: u64 = 0xD6E8_FEB8_6659_FD93;
    for &((x, y, z), state) in applied {
        key ^= x as u32 as u64 | ((z as u32 as u64) << 32);
        key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        key ^= (y as u64) | ((state as u64) << 32);
        key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    }
    key
}

/// Stable identity for a durable native image: position plus journal LSN.
/// A store write (checkpoint or applied snapshot) advances the LSN, so a
/// loaded edit that was flushed and evicted still invalidates.
fn native_key(chunk: &Chunk) -> u64 {
    let mut key: u64 = 0x1656_67B1_9E37_79B9;
    key ^= chunk.pos.x as u32 as u64 | ((chunk.pos.z as u32 as u64) << 32);
    key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    key ^= chunk.world_journal_lsn();
    key = key.wrapping_mul(0x9E37_79B9_7F4A_7C15);
    key
}

fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| i64::try_from(elapsed.as_millis()).unwrap_or(i64::MAX))
        .unwrap_or(0)
}

/// Mint a fresh opaque world epoch: 128 random bits as lowercase hex.
/// Never derived from the seed. Uses the workspace `uuid` dependency
/// (`v4` feature, already on mc-core) — no custom RNG.
pub(crate) fn mint_epoch() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub(crate) fn epoch_for_open(
    tick_state: &crate::tick::TickState,
) -> Result<String, crate::CoreError> {
    if let Some(epoch) = read_epoch(tick_state) {
        return Ok(epoch);
    }
    Ok(mint_epoch())
}

/// NBT path of the persisted epoch: `level.dat` root -> `Data` compound ->
/// namespaced unowned field. `level.dat/Data` is a vanilla compound whose
/// unknown fields survive import, checkpoint refresh and export untouched
/// (see `level_metadata::checkpoint_tags`: only owned keys are written).
/// Never a root-level `level_metadata` record entry: `from_tag` rejects
/// non-compound roots and `validate_globals` rejects unknown domains, so a
/// root String would fail every world's initial checkpoint.
const EPOCH_PATH: &str = "level.dat";
const EPOCH_FIELD: &str = "SolarisTerrainEpoch";

/// Read the persisted epoch from the `level.dat/Data` compound in
/// `native_globals`. Returns `None` when missing or malformed (fresh world
/// or pre-epoch database): the caller mints a fresh epoch, which
/// `stage_epoch` records for the next checkpoint.
fn read_epoch(tick_state: &crate::tick::TickState) -> Option<String> {
    read_data_field(&tick_state.native_globals, EPOCH_FIELD)
        .ok()
        .flatten()
        .filter(|epoch| is_epoch(epoch))
}

/// One unowned string field of `level.dat/Data` in the in-memory globals:
/// `Ok(None)` when the field (or a record/compound holding it) is absent,
/// `Err` when something present on its path is undecodable or mistyped.
pub(crate) fn read_data_field(
    globals: &std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    field: &str,
) -> Result<Option<String>, String> {
    let Some(record) = globals.get("level_metadata") else {
        return Ok(None);
    };
    let tag = crate::native_checkpoint::decode_tag(record, "level_metadata")
        .map_err(|error| format!("level_metadata: {error}"))?;
    let mc_nbt::Tag::Compound(roots) = tag else {
        return Err("level_metadata is not a compound".to_owned());
    };
    let Some(level) = roots
        .iter()
        .find(|(path, _)| path == EPOCH_PATH)
        .map(|(_, tag)| tag)
    else {
        return Ok(None);
    };
    let mc_nbt::Tag::Compound(level) = level else {
        return Err(format!("{EPOCH_PATH} is not a compound"));
    };
    let Some(data) = level
        .iter()
        .find(|(key, _)| key == "Data")
        .map(|(_, tag)| tag)
    else {
        return Ok(None);
    };
    let mc_nbt::Tag::Compound(data) = data else {
        return Err(format!("{EPOCH_PATH}/Data is not a compound"));
    };
    let Some(value) = data
        .iter()
        .find(|(key, _)| key == field)
        .map(|(_, tag)| tag)
    else {
        return Ok(None);
    };
    let mc_nbt::Tag::String(value) = value else {
        return Err(format!("{EPOCH_PATH}/Data/{field} is not a string"));
    };
    Ok(Some(value.clone()))
}

/// Stage a freshly minted epoch into the in-memory `level_metadata` image
/// under `level.dat/Data/SolarisTerrainEpoch`. No-op when the image already
/// carries a valid epoch (reopened world). Called on the opening thread
/// before any tick/journal owner exists; the subsequent initial checkpoint
/// persists the image through the normal atomic transaction.
pub(crate) fn stage_epoch(
    globals: &mut std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    epoch: &str,
) -> Result<(), crate::CoreError> {
    debug_assert!(is_epoch(epoch));
    stage_data_field(globals, EPOCH_FIELD, epoch)
}

/// Stage `value` under `level.dat/Data/<field>` (see [`stage_epoch`]).
pub(crate) fn stage_data_field(
    globals: &mut std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    field: &str,
    epoch: &str,
) -> Result<(), crate::CoreError> {
    let record = globals.get("level_metadata").ok_or_else(|| {
        crate::CoreError::Storage("terrain epoch: missing native level metadata record".into())
    })?;
    let mut tag = crate::native_checkpoint::decode_tag(record, "level_metadata")?;
    let mc_nbt::Tag::Compound(roots) = &mut tag else {
        return Err(crate::CoreError::Storage(
            "terrain epoch: level metadata is not a compound".into(),
        ));
    };
    // Fresh worlds have no converter templates yet. Seed only the unowned
    // epoch field's containers; checkpoint_tags fills the owned level data.
    if !roots.iter().any(|(path, _)| path == EPOCH_PATH) {
        roots.push((
            EPOCH_PATH.into(),
            mc_nbt::Tag::Compound(vec![("Data".into(), mc_nbt::Tag::Compound(Vec::new()))]),
        ));
    }
    let level = roots
        .iter_mut()
        .find(|(path, _)| path == EPOCH_PATH)
        .map(|(_, tag)| tag);
    let mc_nbt::Tag::Compound(level) = level.ok_or_else(|| {
        crate::CoreError::Storage("terrain epoch: missing level.dat template".into())
    })?
    else {
        return Err(crate::CoreError::Storage(
            "terrain epoch: level.dat root is not a compound".into(),
        ));
    };
    let data = level
        .iter_mut()
        .find(|(key, _)| *key == "Data")
        .map(|(_, tag)| tag);
    let mc_nbt::Tag::Compound(data) = data.ok_or_else(|| {
        crate::CoreError::Storage("terrain epoch: missing level.dat Data compound".into())
    })?
    else {
        return Err(crate::CoreError::Storage(
            "terrain epoch: level.dat Data is not a compound".into(),
        ));
    };
    if data.iter().any(|(key, value)| {
        key == field && matches!(value, mc_nbt::Tag::String(have) if have == epoch)
    }) {
        return Ok(());
    }
    data.retain(|(key, _)| key != field);
    data.push((field.into(), mc_nbt::Tag::String(epoch.to_owned())));
    let tick = record.captured_tick;
    globals.insert(
        "level_metadata".into(),
        crate::native_checkpoint::tag_record(tick, &tag)?,
    );
    Ok(())
}

fn is_epoch(value: &str) -> bool {
    value.len() == 32 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}

#[cfg(test)]
#[path = "terrain_tests.rs"]
mod tests;
