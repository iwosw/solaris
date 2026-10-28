//! Resident chunk world: native loading, generation, COW snapshots.
//!
//! The authoritative state is a map of resident chunks plus a dirty-block
//! overlay applied by the block phase. Missing chunks are produced off-tick
//! by [`ChunkLoader`] workers from native coverage or immutable worldgen and
//! become resident at a later tick's input step. Snapshots share unchanged
//! chunks by `Arc`.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::{Arc, atomic::AtomicU64};

use mc_data::Identifier;
use mc_world::native::{NativeChunkSnapshot, NativeStore};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkPos, ChunkSection};

pub use crate::chunk_loader::ChunkLoadError;
use crate::chunk_loader::{ChunkLoader, ChunkSource};
use crate::config::CoreError;
mod capture;
mod clocks;
mod residency;

/// All chunks in a square radius around a center chunk.
#[must_use]
pub fn chunks_in_radius(center: ChunkPos, radius: i32) -> Vec<ChunkPos> {
    let mut out = Vec::new();
    for dz in -radius..=radius {
        for dx in -radius..=radius {
            out.push(ChunkPos {
                x: center.x + dx,
                z: center.z + dz,
            });
        }
    }
    out
}

/// Authoritative resident chunk state on the tick thread.
pub struct ChunkWorld {
    seed: i64,
    source: Arc<ChunkSource>,
    /// Off-tick producers of missing chunks.
    loader: ChunkLoader,
    _air: BlockStateId,
    _biome: Identifier,
    world_spawn: [i32; 3],
    world_spawn_rotation: [f32; 2],
    /// Resident chunks by position.
    chunks: HashMap<ChunkPos, Arc<Chunk>>,
    /// Authoritative per-block writes not yet folded into resident chunks.
    overlay: HashMap<(i32, i32, i32), u32>,
    /// Family changes remove old storage before final block-entity WAL images.
    block_entity_removed: Vec<mc_world::BlockPos>,
    /// Admission already invalidated old storage; folding must preserve a new
    /// block entity installed after that write, even if block edits coalesce.
    block_entity_replaced: HashSet<(i32, i32, i32)>,
    /// Overlay-touched chunk positions, drained by `fold_overlay`.
    touched: Vec<ChunkPos>,
    /// Pending structural edits for villagers that can open wooden doors.
    route_touched: Vec<ChunkPos>,
    /// Chunks whose content or deferred inhabited image is newer than their
    /// native checkpoint (block edits, block-entity storage, counter intervals).
    /// Kept outside `Chunk`, so clearing it never copies a shared snapshot.
    /// Eviction retains them until a checkpoint write is acknowledged for
    /// their current revision ([`Self::mark_flushed`]).
    dirty: HashSet<ChunkPos>,
    /// Per dirty chunk, the block sections (bit = index from the geometry's
    /// `min_y`) edited since its last acknowledged checkpoint. Only
    /// [`Self::fold_overlay`] changes resident block states, so a chunk dirty
    /// for any other reason (light, ticks, block entities, clocks) has no
    /// entry. Cleared with `dirty` by [`Self::mark_flushed`].
    dirty_sections: HashMap<ChunkPos, u32>,
    /// Chunks with block-entity storage changes whose chunk is not
    /// resident (container edited while its chunk was evicted, or the
    /// chunk was never resident). The next checkpoint capture makes them
    /// resident (load-native-or-generate) and writes them; eviction
    /// retains them until flushed.
    dirty_absent: HashSet<ChunkPos>,
    /// 001 slice0 active-only catch-up debt authority (001-d/e): per-chunk
    /// `{ last_sim, remaining }`. `active` membership is parent-managed via
    /// [`Self::set_active_chunks`]; strict empty default (never implicit).
    sim_debt: HashMap<ChunkPos, crate::debt::SimDebtRecord>,
    /// Absolute debt post-images drained by [`Self::journal_debt`]; the next
    /// checkpoint captures the same authority values.
    debt_changes: HashMap<ChunkPos, crate::journal::SimDebtImage>,
    /// Clock boundaries, separate from debt consumption; `None` means eviction
    /// removed authority and its rows need the ordinary absolute reanchor.
    /// Entity mirrors rebase these chunks before `journal_debt` clears them.
    activity_changes: HashMap<ChunkPos, Option<crate::journal::SimDebtImage>>,
    sim_tick: u64,
    /// Parent-managed active membership (hysteresis set). `refresh_active_clock`
    /// reconciles it against residency; cached stationary ticks skip re-sends.
    active_set: HashSet<ChunkPos>,
    /// Level-32 demand, excluding the entity-ticking set.
    block_only: HashSet<ChunkPos>,
    /// Membership fence for lazy entity-clock mirrors. Content revisions and
    /// uninterrupted live accrual do not invalidate those mirrors.
    active_revision: u64,
    inhabited_time: HashMap<ChunkPos, InhabitedCounter>,
    inhabited_tick: u64,
    inhabited_active: Vec<ChunkPos>,
    inhabited_current: Vec<ChunkPos>,
    inhabited_changes: Vec<crate::journal::InhabitedTimeImage>,
    /// Ticket of the one load request currently accepted per position; a
    /// result with any other ticket (superseded or re-requested) is dropped.
    in_flight: HashMap<ChunkPos, u64>,
    /// Loader ticket source, apart from `next_revision`: a refused submit or
    /// a racing load must never shift the stamps of durable chunk revisions.
    next_ticket: u64,
    /// Finished loads wait outside residency until a whole-chunk light solve
    /// fits this tick's admission batch. Tickets remain in `in_flight`.
    ready: Vec<crate::chunk_loader::Ready>,
    /// EWMA of measured admission microseconds per chunk, including its
    /// synchronous light fence. Start conservatively with one whole chunk.
    admission_us_per_chunk: u128,
    /// Shared absolute global-tick deadline; never renewed by dimension calls.
    admission_deadline: Option<std::time::Instant>,
    /// Admission wall time across every ensure call in the current global tick.
    admission_tick_elapsed: std::time::Duration,
    /// Remaining mandatory progress costs, shared by this tick's worlds.
    admission_progress_reservations: Option<Arc<AtomicU64>>,
    /// This world's reservation until its first progress batch settles.
    admission_progress_reserve_us: u64,
    /// Failed stored loads remain unavailable until an operator repairs and restarts.
    failed: HashMap<ChunkPos, ChunkLoadError>,
    /// Durable revisions fence checkpoint ACKs for resident chunks and absent
    /// inhabited images, including private block-entity timers and inventories.
    revisions: HashMap<ChunkPos, u64>,
    next_revision: u64,
    /// Client-visible content revision; private custody does not invalidate
    /// framed chunk packets or the collision/activation footprint.
    wire_revisions: HashMap<ChunkPos, u64>,
    /// Route geometry for door-opening villagers; physical/wire revisions stay separate.
    villager_route_revisions: HashMap<ChunkPos, u64>,
    wooden_doors: Arc<[bool]>,
    /// Pushed POI revision events since the last [`Self::swap_poi_revisions`]
    /// consume. Every `bump_revision` and every resident unload inserts its
    /// chunk (reloads report via `bump_revision`); entries coalesce per chunk
    /// and never couple to queued POI rescans or general tick drains.
    poi_revisions: HashSet<ChunkPos>,
    /// Mutation-owned hydration work, rather than a resident scan every tick.
    hydration_pending: HashSet<ChunkPos>,
    /// Eviction only revisits changed chunks, removed wants and checkpoint ACKs.
    eviction_check: HashSet<ChunkPos>,
    previous_want: HashSet<ChunkPos>,
    /// The last published chunk list; rebuilt after resident storage changes.
    published: Option<Arc<Vec<crate::boundary::SnapshotChunk>>>,
    /// Clean chunks evicted lately, so a player turning back does not pay
    /// for generation again (~20 ms per fresh chunk).
    recent: RecentChunks,
    /// Immutable generated bases awaiting the journal thread's fill-absent
    /// native transaction. Later resident edits copy on write.
    generated: Vec<CapturedChunk>,
    /// Derived light cache owned by this world, never by an edge session.
    pub(crate) light: crate::light::LightEngine,
}

/// Entity-step-local resident-column cache. Borrowing the world prevents edits
/// during its lifetime; it is never retained across entity steps or ticks.
pub(crate) struct PhysicsReader<'a> {
    world: &'a ChunkWorld,
    column: std::cell::Cell<Option<(ChunkPos, Option<&'a Chunk>)>>,
    section: std::cell::Cell<Option<(ChunkPos, i32, &'a ChunkSection)>>,
}

impl PhysicsReader<'_> {
    #[inline]
    pub(crate) fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        if !self.world.overlay.is_empty()
            && let Some(state) = self.world.overlay.get(&(x, y, z))
        {
            return Some(*state);
        }
        let pos = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        let lx = x.rem_euclid(16) as u8;
        let lz = z.rem_euclid(16) as u8;
        // Fast path: same chunk and same 16-high section as a previous read.
        // The borrowed section plus its base Y identifies the cell exactly,
        // so this skips the chunk-map lookup and `world_y_to_section`.
        // Resident chunks are immutable behind `&`, and the cached position
        // keys the entry, so a stale section can never hit.
        if let Some((cached, base, section)) = self.section.get()
            && cached == pos
            && y >= base
            && y < base + 16
        {
            return Some(section.get(lx, (y - base) as u8, lz).0);
        }
        let chunk = match self.column.get() {
            Some((cached, chunk)) if cached == pos => chunk,
            _ => {
                let chunk = self.world.chunks.get(&pos).map(Arc::as_ref);
                self.column.set(Some((pos, chunk)));
                chunk
            }
        }?;
        let Some((index, sy)) = chunk.geometry().world_y_to_section(y) else {
            return Some(self.world._air.0);
        };
        let section = &chunk.sections[index];
        self.section.set(Some((pos, y - i32::from(sy), section)));
        Some(section.get(lx, sy, lz).0)
    }
}

struct InhabitedCounter {
    image: crate::journal::InhabitedTimeImage,
    last_seen: u64,
    dirty: bool,
}

/// Bounded cache of evicted clean chunks, oldest dropped first. Their
/// content equals disk-or-worldgen, so taking one back is exactly a load.
#[derive(Default)]
struct RecentChunks {
    chunks: HashMap<ChunkPos, (Arc<Chunk>, u64)>,
    /// Insertion order; entries whose stamp no longer matches were taken or
    /// re-inserted and are skipped.
    order: std::collections::VecDeque<(ChunkPos, u64)>,
    stamp: u64,
}

/// Evicted chunks kept for a return visit: about 12 view squares of
/// radius 6 (169 chunks each).
const RECENT_CHUNKS: usize = 2048;

/// Tick-owned whole-chunk admission budget. A single chunk is the progress
/// floor even when its measured cost exceeds this budget.
pub(crate) const ADMISSION_BUDGET_US: u128 = 8_000;

/// Generated chunks held for saving while the journal thread is busy (a
/// slow disk); bounds that memory.
const GENERATED_PENDING_MAX: usize = 1024;

impl RecentChunks {
    fn insert(&mut self, pos: ChunkPos, chunk: Arc<Chunk>) {
        self.stamp += 1;
        self.chunks.insert(pos, (chunk, self.stamp));
        self.order.push_back((pos, self.stamp));
        while self.chunks.len() > RECENT_CHUNKS {
            let Some((old, stamp)) = self.order.pop_front() else {
                break;
            };
            if self
                .chunks
                .get(&old)
                .is_some_and(|(_, current)| *current == stamp)
            {
                self.chunks.remove(&old);
            }
        }
        // Stale order entries are bounded too: compact once they dominate.
        if self.order.len() > RECENT_CHUNKS * 2 {
            let chunks = &self.chunks;
            self.order
                .retain(|(pos, stamp)| chunks.get(pos).is_some_and(|(_, s)| s == stamp));
        }
    }

    fn take(&mut self, pos: ChunkPos) -> Option<Arc<Chunk>> {
        self.chunks.remove(&pos).map(|(chunk, _)| chunk)
    }
}

impl std::fmt::Debug for ChunkWorld {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ChunkWorld")
            .field("chunks", &self.chunks.len())
            .field("overlay", &self.overlay.len())
            .finish()
    }
}

/// Memoized section-palette gate for one read-only entity pass.
///
/// Each entry records whether a single resident section's palette matched one
/// fixed predicate. A pass owns one cache per predicate (contact hazards keep
/// a hazard cache and a suffocation cache): `reset` once, then share across
/// every entity probe; `reset` clears entries but keeps capacity. Overlay
/// chunks are seeded once per `reset` so probes never rescan the overlay map;
/// missing columns and overlay-touched chunks fail open to the exact scan
/// (a `true` answer never proves presence; `false` proves absence).
/// Contents are meaningless for any other predicate or after a world write.
#[derive(Default)]
pub(crate) struct SectionMatchCache {
    entries: HashMap<(ChunkPos, usize), bool>,
    overlay_chunks: HashSet<ChunkPos>,
}

impl SectionMatchCache {
    /// Clear per-section results (keeping capacity) and re-seed overlay marks
    /// from the world's current dirty-block overlay.
    pub(crate) fn reset(&mut self, world: &ChunkWorld) {
        self.entries.clear();
        self.overlay_chunks.clear();
        self.overlay_chunks
            .extend(world.overlay.keys().map(|&(x, _, z)| ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            }));
    }
}

/// Exact cell coverage of a previous negative contact probe.
/// Slot-indexed users must also match `entity` after store compaction.
#[derive(Clone, Copy)]
pub(crate) struct ContactStamp<const N: usize = 0> {
    pub(crate) entity: crate::boundary::EntityId,
    pub(crate) position: [f64; 3],
    pub(crate) shape: [f64; 2],
    pub(crate) min: (i32, i32, i32),
    pub(crate) max: (i32, i32, i32),
    /// Extra exact sample cells (contact uses support and eye); not a union box.
    pub(crate) probes: [[i32; 3]; N],
    pub(crate) revision: u64,
}

impl ChunkWorld {
    /// Prove a previous contact-cell read is still current. Chunk revisions
    /// conservatively wake all covered sections; pending overlay edits wake
    /// before fold updates those revisions. `cache` must be reset for this pass.
    pub(crate) fn contact_unchanged_since(
        &self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        since: u64,
        cache: &SectionMatchCache,
    ) -> bool {
        let (x0, x1) = (min.0.div_euclid(16), max.0.div_euclid(16));
        let (z0, z1) = (min.2.div_euclid(16), max.2.div_euclid(16));
        // Empty overlay (the common case) needs no per-chunk overlay probe.
        let overlay = !cache.overlay_chunks.is_empty();
        let quiet = |cx: i32, cz: i32| {
            let pos = ChunkPos { x: cx, z: cz };
            !(overlay && cache.overlay_chunks.contains(&pos))
                && self.wire_revisions.get(&pos).copied().unwrap_or(u64::MAX) <= since
        };
        if x0 == x1 && z0 == z1 {
            // Most bodies sit inside one chunk: one lookup, no iterator nest.
            return quiet(x0, z0);
        }
        (x0..=x1).all(|cx| (z0..=z1).all(|cz| quiet(cx, cz)))
    }

    pub(crate) fn village_sites_in_region(
        &self,
        min_chunk: (i32, i32),
        max_chunk: (i32, i32),
    ) -> Vec<mc_world::GeneratedVillageSite> {
        self.source
            .generator
            .village_sites_in_region(min_chunk, max_chunk)
    }

    /// The shared chunk generator (village sites, noise biomes) for `/locate`.
    pub(crate) fn generator(&self) -> Arc<dyn ChunkGenerator> {
        Arc::clone(&self.source.generator)
    }

    /// World height range for `/locate biome` vertical samples.
    pub(crate) fn geometry(&self) -> mc_world::ChunkGeometry {
        self.source.geometry
    }

    /// Unfolded authoritative block writes, for `/locate poi` scans.
    pub(crate) fn overlay_entries(&self) -> impl Iterator<Item = ((i32, i32, i32), u32)> + '_ {
        self.overlay
            .iter()
            .map(|(&(x, y, z), &state)| ((x, y, z), state))
    }

    pub fn new(
        seed: i64,
        generator: Arc<dyn ChunkGenerator>,
        registry: &crate::registry::CoreRegistry,
        air: BlockStateId,
        world_spawn: [i32; 3],
        geometry: mc_world::ChunkGeometry,
    ) -> Self {
        Self::from_source(
            seed,
            Arc::new(ChunkSource {
                native: None,
                dimension: 0,
                generator,
                registry: Arc::clone(&registry.blocks),
                items: Arc::clone(&registry.items),
                chest_loot: Arc::clone(&registry.chest_loot),
                light: Arc::clone(&registry.light),
                geometry,
            }),
            air,
            world_spawn,
            Arc::clone(&registry.light),
            Arc::clone(&registry.wooden_doors),
        )
    }

    /// Attach the shared all-dimension native store before starting workers.
    /// `new` remains a memory-only constructor, even with a legacy directory.
    #[allow(clippy::too_many_arguments)]
    pub fn new_native(
        seed: i64,
        store: Arc<NativeStore>,
        dimension: u32,
        generator: Arc<dyn ChunkGenerator>,
        registry: &crate::registry::CoreRegistry,
        air: BlockStateId,
        world_spawn: [i32; 3],
        geometry: mc_world::ChunkGeometry,
    ) -> Self {
        Self::from_source(
            seed,
            Arc::new(ChunkSource {
                native: Some(store),
                dimension,
                generator,
                registry: Arc::clone(&registry.blocks),
                items: Arc::clone(&registry.items),
                chest_loot: Arc::clone(&registry.chest_loot),
                light: Arc::clone(&registry.light),
                geometry,
            }),
            air,
            world_spawn,
            Arc::clone(&registry.light),
            Arc::clone(&registry.wooden_doors),
        )
    }

    /// The read-only store + generator workers load from; far-terrain LOD
    /// summaries read through the same source off the tick thread.
    pub(crate) fn chunk_source(&self) -> Arc<ChunkSource> {
        Arc::clone(&self.source)
    }

    fn from_source(
        seed: i64,
        source: Arc<ChunkSource>,
        air: BlockStateId,
        world_spawn: [i32; 3],
        light: Arc<mc_data::block_light::BlockLightTable>,
        wooden_doors: Arc<[bool]>,
    ) -> Self {
        // Half the cores: with one worker per core, exploring players kept
        // generation at the whole CPU budget (3.2 of 6 cores measured, with
        // the edge and tick sharing the rest) and the tick stalled at ~110 ms.
        let workers = (crate::tick::worker_threads() / 2).max(1);
        let loader = ChunkLoader::spawn(Arc::clone(&source), workers);
        Self {
            seed,
            source,
            loader,
            _air: air,
            _biome: Identifier::parse("minecraft:plains").expect("plains parses"),
            world_spawn,
            world_spawn_rotation: [0.0; 2],
            chunks: HashMap::default(),
            overlay: HashMap::default(),
            sim_debt: HashMap::default(),
            debt_changes: HashMap::default(),
            activity_changes: HashMap::default(),
            sim_tick: 0,
            active_set: HashSet::default(),
            block_only: HashSet::default(),
            active_revision: 0,
            block_entity_removed: Vec::new(),
            block_entity_replaced: HashSet::default(),
            touched: Vec::new(),
            route_touched: Vec::new(),
            dirty: HashSet::default(),
            dirty_absent: HashSet::default(),
            dirty_sections: HashMap::default(),
            inhabited_time: HashMap::default(),
            inhabited_tick: 0,
            inhabited_active: Vec::new(),
            inhabited_current: Vec::new(),
            inhabited_changes: Vec::new(),
            in_flight: HashMap::default(),
            next_ticket: 0,
            ready: Vec::new(),
            admission_us_per_chunk: ADMISSION_BUDGET_US,
            admission_deadline: None,
            admission_tick_elapsed: std::time::Duration::ZERO,
            admission_progress_reservations: None,
            admission_progress_reserve_us: 0,
            failed: HashMap::default(),
            revisions: HashMap::default(),
            next_revision: 0,
            wire_revisions: HashMap::default(),
            villager_route_revisions: HashMap::default(),
            wooden_doors,
            poi_revisions: HashSet::default(),
            hydration_pending: HashSet::default(),
            eviction_check: HashSet::default(),
            previous_want: HashSet::default(),
            published: None,
            recent: RecentChunks::default(),
            generated: Vec::new(),
            light: crate::light::LightEngine::new(light),
        }
    }

    /// Generated world spawn as block coordinates; Y is the first free block.
    #[must_use]
    pub fn world_spawn(&self) -> [i32; 3] {
        self.world_spawn
    }

    /// Update the default respawn location captured by the next checkpoint.
    pub(crate) fn set_world_spawn(&mut self, position: [i32; 3], rotation: [f32; 2]) {
        self.world_spawn = position;
        self.world_spawn_rotation = rotation;
    }

    pub(crate) fn world_spawn_rotation(&self) -> [f32; 2] {
        self.world_spawn_rotation
    }

    /// Player feet position centered in the generated spawn column.
    #[must_use]
    pub fn world_spawn_position(&self) -> [f64; 3] {
        [
            f64::from(self.world_spawn[0]) + 0.5,
            f64::from(self.world_spawn[1]),
            f64::from(self.world_spawn[2]) + 0.5,
        ]
    }

    /// Block registry (checkpoint encode use).
    #[must_use]
    pub fn registry(&self) -> &Arc<BlockRegistry> {
        &self.source.registry
    }

    /// New client-visible content, including load, block edits and baked light.
    fn bump_revision(&mut self, pos: ChunkPos) {
        self.bump_wire_revision(pos);
        self.villager_route_revisions
            .insert(pos, self.next_revision);
    }

    fn bump_wire_revision(&mut self, pos: ChunkPos) {
        self.bump_storage_revision(pos);
        self.wire_revisions.insert(pos, self.next_revision);
        self.poi_revisions.insert(pos);
    }

    fn bump_storage_revision(&mut self, pos: ChunkPos) {
        self.next_revision += 1;
        self.revisions.insert(pos, self.next_revision);
        self.published = None;
        self.hydration_pending.insert(pos);
        self.eviction_check.insert(pos);
    }

    /// The revision counter now: every later chunk change gets a larger one.
    #[must_use]
    pub fn revision_now(&self) -> u64 {
        self.next_revision
    }

    /// Content revision of a resident chunk; `u64::MAX` when not resident.
    #[must_use]
    pub fn chunk_revision(&self, cx: i32, cz: i32) -> u64 {
        self.wire_revisions
            .get(&ChunkPos { x: cx, z: cz })
            .copied()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn villager_route_revision(&self, cx: i32, cz: i32) -> u64 {
        self.villager_route_revisions
            .get(&ChunkPos { x: cx, z: cz })
            .copied()
            .unwrap_or(u64::MAX)
    }

    pub(crate) fn pending_villager_route_chunks(&self) -> &[ChunkPos] {
        &self.route_touched
    }

    /// Storage ACK fence for in-memory fixtures which have no checkpoint store.
    #[cfg(test)]
    pub(crate) fn storage_revision(&self, pos: ChunkPos) -> u64 {
        self.revisions.get(&pos).copied().unwrap_or(u64::MAX)
    }
    pub(crate) fn take_hydration_chunks(&mut self) -> Vec<ChunkPos> {
        let mut positions: Vec<_> = self.hydration_pending.drain().collect();
        positions.sort_unstable_by_key(|pos| (pos.x, pos.z));
        positions
    }

    /// Evict resident chunks outside `want`. Dirty chunks (block edits or
    /// container storage changes) are retained until a checkpoint wrote
    /// them — evicting them would lose post-images the journal may already
    /// have pruned past. Returns evicted positions.
    pub fn evict_outside(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        let mut evicted = Vec::new();
        let mut check = std::mem::take(&mut self.eviction_check);
        self.previous_want.retain(|pos| {
            if want.contains_key(pos) {
                true
            } else {
                check.insert(*pos);
                false
            }
        });
        self.previous_want.extend(want.keys().copied());
        for pos in check.drain() {
            if want.contains_key(&pos)
                || self.dirty.contains(&pos)
                || self.sim_debt.get(&pos).is_some_and(|debt| debt.dirty)
                || self
                    .inhabited_time
                    .get(&pos)
                    .is_some_and(|counter| counter.dirty || counter.image.active)
            {
                continue;
            }
            if let Some(chunk) = self.chunks.remove(&pos) {
                evicted.push(pos);
                self.recent.insert(pos, chunk);
            }
        }
        self.eviction_check = check;
        for pos in &evicted {
            self.revisions.remove(pos);
            self.wire_revisions.remove(pos);
            self.villager_route_revisions.remove(pos);
            self.inhabited_time.remove(pos);
            self.sim_debt.remove(pos);
            self.activity_changes.entry(*pos).or_insert(None);
            self.light.remove(*pos);
            self.poi_revisions.insert(*pos);
        }
        if !evicted.is_empty() {
            self.active_revision += 1;
            self.published = None;
        }
        evicted
    }

    /// Mark private block-entity custody dirty without changing wire content.
    /// A fresh durable revision still fences stale checkpoint acknowledgements.
    /// Client-visible block-entity owners separately mark their public update.
    pub fn mark_block_entity_dirty(&mut self, pos: mc_world::BlockPos) {
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if self.chunks.contains_key(&chunk) {
            self.dirty.insert(chunk);
            self.bump_storage_revision(chunk);
        } else {
            self.dirty_absent.insert(chunk);
        }
    }

    /// A public block-entity update changes the payload of future chunk sends,
    /// even if there are currently no viewers to receive BlockEntityData.
    pub(crate) fn mark_block_entity_visible(&mut self, pos: mc_world::BlockPos) {
        let chunk = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if self.chunks.contains_key(&chunk) {
            self.bump_revision(chunk);
        }
    }

    /// Inject native scheduler post-images at checkpoint capture. This writes
    /// the chunk's real vanilla scheduled-work field, not a block entity.
    pub(crate) fn set_scheduled_block_ticks(
        &mut self,
        pos: ChunkPos,
        ticks: Vec<mc_world::ScheduledBlockTick>,
    ) {
        let Some(chunk) = self.chunks.get_mut(&pos) else {
            return;
        };
        if chunk.scheduled_block_ticks() == ticks {
            return;
        }
        let chunk = Arc::make_mut(chunk);
        chunk.load_scheduled_block_ticks(ticks);
        chunk.mark_dirty();
        self.dirty.insert(pos);
        self.bump_storage_revision(pos);
    }

    /// Freeze metadata now; reading and preparing an absent chunk belongs to
    /// the journal thread after the WAL gate, never to checkpoint capture.
    /// Perf fence: checkpoint-restart spent ~159ms loading/baking on-tick.
    /// Deferral cut total capture to ~3.3ms (.analysis/codex-logs/tick-perf).
    fn defer_checkpoint_chunk<'a>(
        &mut self,
        pos: ChunkPos,
        capture: &'a mut ChunkCapture,
    ) -> Option<&'a mut DeferredChunkCapture> {
        if let Some(error) = self.failed.get(&pos) {
            capture.load_error = Some(error.clone());
            return None;
        }
        self.in_flight.remove(&pos);
        Some(
            capture
                .deferred
                .entry(pos)
                .or_insert_with(|| DeferredChunkCapture {
                    revision: self.revisions.get(&pos).copied().unwrap_or(0),
                    cached: self.recent.take(pos),
                    inhabited: None,
                    debt: None,
                    block_ticks: None,
                    fluid_ticks: None,
                }),
        )
    }

    /// Capture unavailable scheduled-work cells without making their chunks
    /// simulation-resident. Checkpoint I/O must not activate an overdue cell.
    pub(crate) fn capture_scheduled_block_ticks(
        &mut self,
        capture: &mut ChunkCapture,
        absent: Vec<(ChunkPos, Vec<mc_world::ScheduledBlockTick>)>,
    ) {
        if capture.source.native.is_none() {
            return;
        }
        for (pos, ticks) in absent {
            // Other checkpoint domains may have materialized this chunk
            // since the scheduler's initial resident/absent partition.
            if self.chunks.contains_key(&pos) {
                capture.deferred.remove(&pos);
                self.set_scheduled_block_ticks(pos, ticks);
                let image = self.captured_chunk(pos);
                if let Some(captured) = capture.chunks.iter_mut().find(|entry| entry.pos == pos) {
                    *captured = image;
                } else {
                    capture.chunks.push(image);
                }
            } else if let Some(deferred) = self.defer_checkpoint_chunk(pos, capture) {
                deferred.block_ticks = Some(ticks);
            }
        }
        capture
            .chunks
            .sort_by_key(|entry| (entry.pos.x, entry.pos.z));
    }

    pub(crate) fn set_scheduled_fluid_ticks(
        &mut self,
        pos: ChunkPos,
        ticks: Vec<mc_world::ScheduledFluidTick>,
    ) {
        let Some(chunk) = self.chunks.get_mut(&pos) else {
            return;
        };
        if chunk.scheduled_fluid_ticks() == ticks {
            return;
        }
        let chunk = Arc::make_mut(chunk);
        chunk.load_scheduled_fluid_ticks(ticks);
        chunk.mark_dirty();
        self.dirty.insert(pos);
        self.bump_storage_revision(pos);
    }

    /// Compose with block-work captures of the same absent chunk; never load
    /// a second image that would overwrite its other scheduler or storage.
    pub(crate) fn capture_scheduled_fluid_ticks(
        &mut self,
        capture: &mut ChunkCapture,
        absent: Vec<(ChunkPos, Vec<mc_world::ScheduledFluidTick>)>,
    ) {
        if capture.source.native.is_none() {
            return;
        }
        for (pos, ticks) in absent {
            if self.chunks.contains_key(&pos) {
                capture.deferred.remove(&pos);
                self.set_scheduled_fluid_ticks(pos, ticks);
                let image = self.captured_chunk(pos);
                if let Some(entry) = capture.chunks.iter_mut().find(|entry| entry.pos == pos) {
                    *entry = image;
                } else {
                    capture.chunks.push(image);
                }
            } else if let Some(deferred) = self.defer_checkpoint_chunk(pos, capture) {
                deferred.fluid_ticks = Some(ticks);
            }
        }
        capture
            .chunks
            .sort_by_key(|entry| (entry.pos.x, entry.pos.z));
    }

    /// Store opaque vanilla block-entity NBT in its checkpointed chunk.
    pub(crate) fn set_opaque_block_entity(
        &mut self,
        pos: mc_world::BlockPos,
        bytes: Option<Vec<u8>>,
    ) {
        let chunk_pos = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if !self.materialize(chunk_pos) {
            return;
        }
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            let chunk = Arc::make_mut(chunk);
            // A brewing stand promoted to opaque native owner state must not
            // retain its imported typed shadow (Anvil serializes typed first).
            chunk.brewing_stands.remove(&pos);
            match bytes {
                Some(bytes) => {
                    chunk.block_entities.insert(pos, bytes);
                }
                None => {
                    chunk.block_entities.remove(&pos);
                }
            }
        }
        self.mark_block_entity_dirty(pos);
    }
    /// Replace imported shadows as well as opaque data when a command supplies a new image.
    pub(crate) fn install_command_block_entity(&mut self, pos: mc_world::BlockPos, bytes: Vec<u8>) {
        let chunk_pos = ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        };
        if !self.materialize(chunk_pos) {
            return;
        }
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            let chunk = Arc::make_mut(chunk);
            chunk.chests.remove(&pos);
            chunk.furnaces.remove(&pos);
            chunk.hoppers.remove(&pos);
            chunk.brewing_stands.remove(&pos);
            chunk.block_entities.insert(pos, bytes);
        }
        self.hydration_pending.insert(chunk_pos);
        self.mark_block_entity_dirty(pos);
    }
    /// Whether the chunk column `(cx, cz)` is resident (and legible).
    /// Unloaded columns block entity movement and never form a floor.
    #[must_use]
    pub fn has_chunk(&self, cx: i32, cz: i32) -> bool {
        self.chunks.contains_key(&ChunkPos { x: cx, z: cz })
    }

    /// 001 slice0 + radius split (roadmap C/G5): simulation gate.
    /// Single debt authority: `sim_debt.active` implies residency (eviction
    /// removes the record; quarantine clears it at its mutation boundary).
    /// Strict empty default: newly resident chunks are never implicitly
    /// active. `has_chunk` stays pure residency.
    #[must_use]
    pub fn is_active(&self, cx: i32, cz: i32) -> bool {
        self.sim_debt
            .get(&ChunkPos { x: cx, z: cz })
            .is_some_and(|debt| debt.active)
    }

    /// Blocks also tick in resident level-32 neighbours of forced tickets.
    #[must_use]
    pub fn is_block_active(&self, cx: i32, cz: i32) -> bool {
        self.is_active(cx, cz)
            || (self.block_only.contains(&ChunkPos { x: cx, z: cz }) && self.has_chunk(cx, cz))
    }

    /// Match the active-position worklist and its authoritative simulation gate.
    #[inline]
    pub(crate) fn is_active_position(&self, cx: i32, cz: i32) -> bool {
        self.active_set.contains(&ChunkPos { x: cx, z: cz }) && self.is_active(cx, cz)
    }

    /// Fence for active/resident membership changes, not ordinary block edits.
    #[must_use]
    pub fn active_revision(&self) -> u64 {
        self.active_revision
    }

    /// Whether the dirty-block overlay is empty (cache fast path).
    #[must_use]
    pub fn overlay_is_empty(&self) -> bool {
        self.overlay.is_empty()
    }

    /// Chunks with unfurled block edits visible to this tick's resident owners.
    pub(crate) fn pending_block_chunks(&self) -> &[ChunkPos] {
        &self.touched
    }

    /// Swap the pending POI revision event set into the caller's buffer.
    /// Clears `changes`, then exchanges it with the world's accumulation, so
    /// the caller owns every event since the previous consume and the world
    /// restarts empty. The parent allocator tick consumes once before its POI
    /// cache refresh, merging [`Self::pending_block_chunks`] (pre-fold edits)
    /// alongside. Reporting never touches `active_revision` semantics.
    pub(crate) fn swap_poi_revisions(&mut self, changes: &mut HashSet<ChunkPos>) {
        changes.clear();
        std::mem::swap(changes, &mut self.poi_revisions);
    }

    /// Read one block state id: overlay first, then the resident chunk.
    #[must_use]
    pub fn block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        // Folded every tick, so empty while entities sample collisions.
        if !self.overlay.is_empty()
            && let Some(state) = self.overlay.get(&(x, y, z))
        {
            return Some(*state);
        }
        let chunk = self.chunks.get(&ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        })?;
        chunk
            .get_block(x.rem_euclid(16) as u8, y, z.rem_euclid(16) as u8)
            .map(|id| id.0)
    }

    /// Cached section-palette gate: whether an inclusive block box can hold
    /// any state matching one fixed predicate over one read-only pass.
    ///
    /// Each resident `(chunk, section)` probes its palette at most once: hits
    /// reuse the cached verdict, misses evaluate the predicate over the
    /// section's palette (or its single state) and memoize that verdict.
    /// Overlay-touched chunks (seeded by [`SectionMatchCache::reset`]) and
    /// missing columns fail open to `true` without probing; out-of-geometry
    /// spans are skipped. A `false` answer proves absence. The cache is
    /// valid only while the predicate and the world stay unchanged.
    #[must_use]
    pub(crate) fn may_hold_matching_state_cached(
        &self,
        min: (i32, i32, i32),
        max: (i32, i32, i32),
        cache: &mut SectionMatchCache,
        mut matches: impl FnMut(BlockStateId) -> bool,
    ) -> bool {
        if min.0 > max.0 || min.1 > max.1 || min.2 > max.2 {
            return false;
        }
        let min_cx = min.0.div_euclid(16);
        let max_cx = max.0.div_euclid(16);
        let min_cz = min.2.div_euclid(16);
        let max_cz = max.2.div_euclid(16);
        // Bound chunk iteration: extreme block spans still visit only their
        // covered columns; per-column geometry clipping bounds section work.
        for cx in min_cx..=max_cx {
            for cz in min_cz..=max_cz {
                let pos = ChunkPos { x: cx, z: cz };
                if cache.overlay_chunks.contains(&pos) {
                    return true;
                }
                let Some(chunk) = self.chunks.get(&pos) else {
                    return true;
                };
                let geometry = chunk.geometry();
                let lo = min.1.max(geometry.min_y());
                let hi = max.1.min(geometry.max_y() - 1);
                if lo > hi {
                    continue;
                }
                let lo_idx = ((lo - geometry.min_y()) / 16) as usize;
                let hi_idx = ((hi - geometry.min_y()) / 16) as usize;
                for (offset, section) in chunk.sections[lo_idx..=hi_idx].iter().enumerate() {
                    let key = (pos, lo_idx + offset);
                    if let Some(hit) = cache.entries.get(&key) {
                        if *hit {
                            return true;
                        }
                        continue;
                    }
                    let hit = if let Some(palette) = section.palette() {
                        palette.iter().copied().any(&mut matches)
                    } else {
                        matches(section.get(0, 0, 0))
                    };
                    cache.entries.insert(key, hit);
                    if hit {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Committed-but-unflushed terrain for off-tick sampling: resident
    /// dirty chunks (position, wire revision, chunk) plus non-resident
    /// overlay writes. Eviction retains dirty chunks and fold retains
    /// non-resident writes, so both survive here until the checkpoint ACK
    /// clears them. Called by the tick owner at publication.
    pub(crate) fn dirty_supplement_data(&self) -> crate::terrain::DirtySupplement {
        let mut chunks = Vec::new();
        for pos in &self.dirty {
            if let Some(chunk) = self.chunks.get(pos) {
                chunks.push((
                    *pos,
                    self.wire_revisions.get(pos).copied().unwrap_or(0),
                    Arc::clone(chunk),
                ));
            }
        }
        chunks.sort_unstable_by_key(|(pos, _, _)| (pos.x, pos.z));
        // No HashSet: the resident map itself answers membership, avoiding
        // an O(resident) allocate+copy per publication. Sampling may
        // allocate off-tick (bounded to 36 chunks).
        let mut overlay: Vec<((i32, i32, i32), u32)> = self
            .overlay
            .iter()
            .filter(|((x, _, z), _)| {
                !self.chunks.contains_key(&ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                })
            })
            .map(|(&(x, y, z), &state)| ((x, y, z), state))
            .collect();
        overlay.sort_unstable();
        crate::terrain::DirtySupplement { chunks, overlay }
    }

    /// Fresh per-step cache; missing columns and overlay writes remain exact.
    pub(crate) fn physics_reader(&self) -> PhysicsReader<'_> {
        PhysicsReader {
            world: self,
            column: std::cell::Cell::new(None),
            section: std::cell::Cell::new(None),
        }
    }

    /// A loaded column ends in air, not an invisible collision floor.
    /// Missing columns remain unknown so ordinary admission still fails closed.
    pub(crate) fn physics_block_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.block_at(x, y, z).or_else(|| {
            let chunk = self.chunks.get(&ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            })?;
            let geometry = chunk.geometry();
            (!(geometry.min_y()..geometry.max_y()).contains(&y)).then_some(self._air.0)
        })
    }

    /// Borrow the resident chunk's quart-resolution biome at a block position.
    #[must_use]
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> Option<&Identifier> {
        let chunk = self.chunks.get(&ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        })?;
        let geometry = chunk.geometry();
        if !(geometry.min_y()..geometry.max_y()).contains(&y) {
            return None;
        }
        let section = ((y - geometry.min_y()) / 16) as usize;
        Some(chunk.biomes.get(section)?.get(
            (x.rem_euclid(16) / 4) as u8,
            (y.rem_euclid(16) / 4) as u8,
            (z.rem_euclid(16) / 4) as u8,
        ))
    }

    fn wooden_door_toggle(&self, previous: Option<u32>, state: u32) -> bool {
        self.wooden_doors
            .get(state as usize)
            .copied()
            .unwrap_or(false)
            && previous
                .and_then(|id| self.source.registry.by_id(BlockStateId(id)))
                .zip(self.source.registry.by_id(BlockStateId(state)))
                .is_some_and(|(before, after)| {
                    before.block.id == after.block.id
                        && before
                            .properties
                            .iter()
                            .zip(&after.properties)
                            .all(|(a, b)| a == b || (a.0 == "open" && b.0 == "open"))
                })
    }

    /// Apply one authoritative block write.
    pub fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        self.cancel_simulation_debt(ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        });
        self.set_block_catch_up(x, y, z, state);
    }

    /// Only the tick merge owner's catch-up transaction bypasses 001-f.
    pub(crate) fn set_block_catch_up(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let previous = self.block_at(x, y, z);
        if previous == Some(state) {
            return;
        }
        let chunk = ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        };
        if self.failed.contains_key(&chunk) {
            return;
        }
        let same_family = previous
            .and_then(|old| self.source.registry.by_id(BlockStateId(old)))
            .zip(self.source.registry.by_id(BlockStateId(state)))
            .is_some_and(|(old, new)| {
                old.block.id == new.block.id
                    || crate::copper::same_family(&old.block.id, &new.block.id)
            });
        if !same_family && let Some(resident) = self.chunks.get_mut(&chunk) {
            self.block_entity_replaced.insert((x, y, z));
            let cell = mc_world::BlockPos { x, y, z };
            let removed = resident.block_entities.contains_key(&cell)
                || resident.hoppers.contains_key(&cell)
                || resident.chests.contains_key(&cell)
                || resident.furnaces.contains_key(&cell)
                || resident.brewing_stands.contains_key(&cell);
            if removed {
                let resident = Arc::make_mut(resident);
                resident.block_entities.remove(&cell);
                resident.hoppers.remove(&cell);
                resident.chests.remove(&cell);
                resident.furnaces.remove(&cell);
                resident.brewing_stands.remove(&cell);
                self.block_entity_removed.push(cell);
                self.dirty.insert(chunk);
                self.bump_revision(chunk);
            }
        }
        if !self.wooden_door_toggle(previous, state) && !self.route_touched.contains(&chunk) {
            self.route_touched.push(chunk);
        }
        self.overlay.insert((x, y, z), state);
        if !self.touched.contains(&chunk) {
            self.touched.push(chunk);
        }
    }

    /// Fold the overlay into resident chunks (commit step).
    pub fn fold_overlay(&mut self) {
        if self.overlay.is_empty() {
            self.touched.clear();
            self.route_touched.clear();
            return;
        }
        let mut revised = HashSet::default();
        for ((x, y, z), state) in std::mem::take(&mut self.overlay) {
            let pos = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            let Some(chunk) = self.chunks.get_mut(&pos) else {
                // Journal replay or eviction: retain writes until admission.
                self.overlay.insert((x, y, z), state);
                continue;
            };
            let replaced_at_admission = self.block_entity_replaced.remove(&(x, y, z));
            let lx = x.rem_euclid(16) as u8;
            let lz = z.rem_euclid(16) as u8;
            let Some(previous) = chunk.get_block(lx, y, lz) else {
                continue;
            };
            if previous.0 == state {
                continue;
            }
            let chunk = Arc::make_mut(chunk);
            let same_family = self
                .source
                .registry
                .by_id(previous)
                .zip(self.source.registry.by_id(BlockStateId(state)))
                .is_some_and(|(old, new)| {
                    old.block.id == new.block.id
                        || crate::copper::same_family(&old.block.id, &new.block.id)
                });
            if !same_family && !replaced_at_admission {
                let cell = mc_world::BlockPos { x, y, z };
                let removed = chunk.block_entities.remove(&cell).is_some()
                    | chunk.hoppers.remove(&cell).is_some()
                    | chunk.chests.remove(&cell).is_some()
                    | chunk.furnaces.remove(&cell).is_some()
                    | chunk.brewing_stands.remove(&cell).is_some();
                if removed {
                    self.block_entity_removed.push(cell);
                }
            }
            chunk.set_block(lx, y, lz, BlockStateId(state));
            if let Some((section, _)) = chunk.geometry().world_y_to_section(y) {
                *self.dirty_sections.entry(pos).or_default() |= 1 << section;
            }
            self.dirty.insert(pos);
            revised.insert(pos);
            // Observe the exact post-image of this edit; batching all block
            // writes before BFS would lose intermediate sky-column changes.
            let updates = self
                .light
                .edit(&self.chunks, (x, y, z), previous, BlockStateId(state));
            revised.extend(self.install_light(updates));
        }
        for pos in revised {
            self.bump_wire_revision(pos);
            if self.route_touched.contains(&pos) {
                self.villager_route_revisions
                    .insert(pos, self.next_revision);
            }
        }
        self.touched.clear();
        self.route_touched.clear();
    }

    pub(crate) fn take_block_entity_removals(&mut self) -> Vec<mc_world::BlockPos> {
        std::mem::take(&mut self.block_entity_removed)
    }

    fn relight_admitted(&mut self, loaded: &[ChunkPos], blocking: bool) {
        if loaded.is_empty() {
            return;
        }
        let updates = if blocking {
            self.light.admit_blocking(&self.chunks, loaded)
        } else {
            self.light.admit(&self.chunks, loaded)
        };
        for pos in self.install_light(updates) {
            self.bump_revision(pos);
        }
    }

    fn install_light(
        &mut self,
        updates: Vec<(ChunkPos, mc_world::light::ChunkLight)>,
    ) -> Vec<ChunkPos> {
        let mut revised = Vec::new();
        for (pos, light) in updates {
            let Some(chunk) = self.chunks.get_mut(&pos) else {
                continue;
            };
            let changed = chunk
                .section_lights
                .iter()
                .enumerate()
                .any(|(index, section)| {
                    section.sky.as_ref() != light.sky.section(index)
                        || section.block.as_ref() != light.block.section(index)
                });
            if changed {
                Arc::make_mut(chunk).set_baked_light(&light);
                self.dirty.insert(pos);
                revised.push(pos);
            }
        }
        revised
    }

    /// Raw sky/block channels. Day and weather attenuation belongs to gameplay,
    /// not these authoritative arrays (or the client would darken them twice).
    #[must_use]
    pub fn light_at(&self, x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
        let chunk = self.chunk(ChunkPos {
            x: x.div_euclid(16),
            z: z.div_euclid(16),
        })?;
        let local_y = usize::try_from(y.checked_sub(chunk.geometry().min_y())?).ok()?;
        let section = chunk.section_lights.get(local_y / 16)?;
        let cell =
            (local_y % 16) * 256 + z.rem_euclid(16) as usize * 16 + x.rem_euclid(16) as usize;
        Some((
            section.sky.as_ref().map_or(0, |sky| sky.get(cell)),
            section.block.as_ref().map_or(0, |block| block.get(cell)),
        ))
    }

    /// Immutable snapshot of the resident chunks, sorted by `(x, z)`;
    /// shared until a chunk becomes resident, changes or is evicted.
    pub fn snapshot(&mut self) -> Arc<Vec<crate::boundary::SnapshotChunk>> {
        if let Some(published) = &self.published {
            return Arc::clone(published);
        }
        let mut out: Vec<crate::boundary::SnapshotChunk> = self
            .chunks
            .iter()
            .map(|(pos, chunk)| crate::boundary::SnapshotChunk {
                pos: *pos,
                revision: self.wire_revisions.get(pos).copied().unwrap_or(0),
                chunk: Arc::clone(chunk),
            })
            .collect();
        out.sort_by_key(|entry| (entry.pos.x, entry.pos.z));
        let published = Arc::new(out);
        self.published = Some(Arc::clone(&published));
        published
    }

    /// Number of resident chunks.
    #[must_use]
    pub fn resident_count(&self) -> usize {
        self.chunks.len()
    }

    /// Resident chunk by position, if loaded.
    #[must_use]
    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos).map(Arc::as_ref)
    }

    /// Positions of all resident chunks.
    #[must_use]
    pub fn resident_positions(&self) -> Vec<ChunkPos> {
        self.chunks.keys().copied().collect()
    }

    /// Resident chunks, in no particular order.
    pub fn chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values().map(Arc::as_ref)
    }

    /// World seed (journal/checkpoint use).
    #[must_use]
    pub fn seed(&self) -> i64 {
        self.seed
    }
}

/// One captured chunk: position, the revision it was captured at, content, and
/// the block sections edited since its last acknowledged checkpoint (`None`:
/// unknown, examine all). The mask is a superset of what changed relative to
/// durable overlays: it keeps bits already written by an unacknowledged capture.
struct CapturedChunk {
    pos: ChunkPos,
    revision: u64,
    chunk: Arc<Chunk>,
    dirty_sections: Option<u32>,
}

/// Frozen capture-only post-images. `None` preserves that native domain;
/// `Some(Vec::new())` clears it. These chunks never enter simulation residency.
struct DeferredChunkCapture {
    revision: u64,
    cached: Option<Arc<Chunk>>,
    inhabited: Option<crate::journal::InhabitedTimeImage>,
    debt: Option<crate::journal::SimDebtImage>,
    block_ticks: Option<Vec<mc_world::ScheduledBlockTick>>,
    fluid_ticks: Option<Vec<mc_world::ScheduledFluidTick>>,
}

/// Dirty chunks captured for one checkpoint: immutable, written off the
/// tick thread (the journal thread, after the WAL gate).
pub struct ChunkCapture {
    source: Arc<ChunkSource>,
    chunks: Vec<CapturedChunk>,
    deferred: HashMap<ChunkPos, DeferredChunkCapture>,
    load_error: Option<ChunkLoadError>,
    generated_bases: bool,
}

impl ChunkCapture {
    pub(crate) fn load_error(&self) -> Option<&ChunkLoadError> {
        self.load_error.as_ref()
    }

    pub fn dimension(&self) -> u32 {
        self.source.dimension
    }

    pub fn registry(&self) -> &Arc<BlockRegistry> {
        &self.source.registry
    }

    pub fn items(&self) -> &Arc<mc_data::items::ItemRegistry> {
        &self.source.items
    }

    pub fn store(&self) -> Option<&Arc<NativeStore>> {
        self.source.native.as_ref()
    }

    /// ACK only after the parent's all-dimension checkpoint commit succeeds.
    pub fn written(&self) -> Vec<(ChunkPos, u64)> {
        let mut written: Vec<_> = self
            .chunks
            .iter()
            .map(|captured| (captured.pos, captured.revision))
            .chain(
                self.deferred
                    .iter()
                    .map(|(pos, image)| (*pos, image.revision)),
            )
            .collect();
        written.sort_unstable_by_key(|(pos, _)| (pos.x, pos.z));
        written
    }

    /// Resolve and consume one frozen baseline/effective pair at a time.
    /// Off-tick preparation must not retain owned terrain for the entire
    /// captured world while the atomic native transaction is being written.
    pub fn visit_native(
        &self,
        mut visit: impl FnMut(NativeChunkSnapshot<'_>) -> Result<(), CoreError>,
    ) -> Result<(), CoreError> {
        if let Some(error) = &self.load_error {
            return Err(CoreError::Storage(error.to_string()));
        }
        for captured in &self.chunks {
            let (pos, chunk) = (&captured.pos, &captured.chunk);
            let base = self
                .source
                .resolve_base(*pos)
                .map_err(|error| CoreError::Storage(error.to_string()))?;
            visit(NativeChunkSnapshot {
                dimension: self.dimension(),
                base: &base,
                effective: chunk,
                dirty_sections: captured.dirty_sections,
            })?;
        }
        let mut absent: Vec<_> = self.deferred.iter().collect();
        absent.sort_unstable_by_key(|(pos, _)| (pos.x, pos.z));
        for (pos, image) in absent {
            let (mut chunk, generated) = match &image.cached {
                Some(chunk) => (chunk.as_ref().clone(), false),
                None => self
                    .source
                    .load_or_generate(*pos)
                    .map_err(|error| CoreError::Storage(error.to_string()))?,
            };
            // Preserve a generated immutable baseline before applying metadata.
            let base = if generated {
                chunk.clone()
            } else {
                self.source
                    .resolve_base(*pos)
                    .map_err(|error| CoreError::Storage(error.to_string()))?
            };
            if let Some(inhabited) = image.inhabited {
                inhabited.apply(&mut chunk, inhabited.tick);
            }
            if let Some(debt) = image.debt {
                crate::debt::project(&mut chunk, debt.last_sim, debt.remaining)?;
            }
            if let Some(ticks) = &image.block_ticks {
                chunk.load_scheduled_block_ticks(ticks.clone());
            }
            if let Some(ticks) = &image.fluid_ticks {
                chunk.load_scheduled_fluid_ticks(ticks.clone());
            }
            chunk.mark_dirty();
            visit(NativeChunkSnapshot {
                dimension: self.dimension(),
                base: &base,
                effective: &chunk,
                dirty_sections: Some(0),
            })?;
        }
        Ok(())
    }

    /// Generated captures contain only the pre-edit immutable images queued
    /// at admission. This never writes effective state or gameplay ACKs.
    pub fn fill_generated(self) -> Result<(), CoreError> {
        if let Some(error) = &self.load_error {
            return Err(CoreError::Storage(error.to_string()));
        }
        if !self.generated_bases {
            return Err(CoreError::Storage(
                "generated save requires an immutable generation capture".into(),
            ));
        }
        let Some(store) = &self.source.native else {
            return Ok(());
        };
        let bases: Vec<_> = self
            .chunks
            .into_iter()
            .map(|captured| Arc::unwrap_or_clone(captured.chunk))
            .collect();
        store
            .fill_generated(
                self.source.dimension,
                &bases,
                &self.source.registry,
                &self.source.items,
            )
            .map_err(|error| CoreError::Storage(error.to_string()))
    }

    /// Standalone fixture checkpoint; production commits belong to the parent.
    #[cfg(test)]
    pub fn write(&self) -> Result<Vec<(ChunkPos, u64)>, CoreError> {
        world_tests::write_capture(self)
    }
}

#[cfg(test)]
#[path = "world_tests.rs"]
pub(crate) mod world_tests;

#[cfg(test)]
#[path = "world_section_dirty_tests.rs"]
mod world_section_dirty_tests;
