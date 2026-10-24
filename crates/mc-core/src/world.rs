//! Resident chunk world: native loading, generation, COW snapshots.
//!
//! The authoritative state is a map of resident chunks plus a dirty-block
//! overlay applied by the block phase. Missing chunks are produced off-tick
//! by [`ChunkLoader`] workers from native coverage or immutable worldgen and
//! become resident at a later tick's input step. Snapshots share unchanged
//! chunks by `Arc`.

use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

use mc_data::Identifier;
use mc_world::native::{NativeChunkSnapshot, NativeStore};
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkGenerator, ChunkPos, ChunkSection};

pub use crate::chunk_loader::ChunkLoadError;
use crate::chunk_loader::{ChunkLoader, ChunkSource};
use crate::config::CoreError;

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
const ADMISSION_BUDGET_US: u128 = 8_000;

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

    /// Join finished off-tick loads for wanted chunks and request the
    /// missing ones, lowest priority value first (`want` maps position →
    /// load priority). Worker queues are bounded, so they never hold
    /// positions that went stale long ago. Never
    /// blocks; returns positions that became resident.
    pub fn ensure(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        let admission_start = std::time::Instant::now();
        let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
            .is_some()
            .then(std::time::Instant::now);
        let loaded = self.admit_ready_limit(want, self.admission_limit(), false);
        self.relight_admitted(&loaded, false);
        self.record_admission_cost(admission_start.elapsed().as_micros(), loaded.len());
        if let Some(start) = phase_start
            && !loaded.is_empty()
        {
            eprintln!(
                "chunk admission ms: loaded={} ready={} pending={} total={:.2}",
                loaded.len(),
                self.ready.len(),
                self.loader.pending(),
                start.elapsed().as_secs_f64() * 1000.0,
            );
        }
        loaded
    }

    fn admission_limit(&self) -> usize {
        (ADMISSION_BUDGET_US / self.admission_us_per_chunk)
            .clamp(1, crate::chunk_loader::READY_LOADS_MAX as u128) as usize
    }

    fn record_admission_cost(&mut self, elapsed_us: u128, loaded: usize) {
        if loaded == 0 {
            return;
        }
        let sample = elapsed_us.div_ceil(loaded as u128).max(1);
        self.admission_us_per_chunk = (self.admission_us_per_chunk * 3 + sample).div_ceil(4);
    }

    /// Admit loaded chunks strictly in `(priority, position)` order: stop at
    /// the first wanted chunk whose load has not finished, so the admission
    /// (and revision stamp) order never depends on worker completion order.
    fn admit_ready(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        self.admit_ready_limit(want, usize::MAX, true)
    }

    fn admit_ready_limit(
        &mut self,
        want: &HashMap<ChunkPos, i32>,
        limit: usize,
        ordered: bool,
    ) -> Vec<ChunkPos> {
        self.ready.extend(self.loader.drain());
        // Discard stale results without consuming the admission budget. Do not
        // clear a newer ticket when an older completion finally arrives.
        let mut index = 0;
        while index < self.ready.len() {
            let (pos, ticket, _) = &self.ready[index];
            if self.in_flight.get(pos) != Some(ticket)
                || !want.contains_key(pos)
                || self.chunks.contains_key(pos)
            {
                let (pos, ticket, result) = self.ready.swap_remove(index);
                self.accept(pos, ticket, result, want);
            } else {
                index += 1;
            }
        }
        let mut missing: Vec<(i32, ChunkPos)> = want
            .iter()
            .filter(|(pos, _)| !self.chunks.contains_key(pos) && !self.failed.contains_key(pos))
            .map(|(pos, priority)| (*priority, *pos))
            .collect();
        missing.sort_unstable_by_key(|(priority, pos)| (*priority, pos.x, pos.z));
        let mut loaded = Vec::new();
        for (_, pos) in &missing {
            if loaded.len() == limit {
                break;
            }
            if let Some(index) = self.ready.iter().position(|entry| entry.0 == *pos) {
                let (pos, ticket, result) = self.ready.swap_remove(index);
                if self.accept(pos, ticket, result, want) {
                    loaded.push(pos);
                }
            } else if let Some(mut chunk) = self.recent.take(*pos) {
                if !self.apply_inhabited_image(*pos, &mut chunk) {
                    continue;
                }
                self.chunks.insert(*pos, chunk);
                self.active_revision += 1;
                self.bump_revision(*pos);
                loaded.push(*pos);
            } else if ordered {
                break;
            }
        }
        // Ready, running and queued jobs share one bound. Recent hits wait in
        // their existing bounded cache and never dispatch another load.
        for (_, pos) in missing {
            if self.chunks.contains_key(&pos)
                || self.in_flight.contains_key(&pos)
                || self.failed.contains_key(&pos)
                || self.recent.chunks.contains_key(&pos)
            {
                continue;
            }
            let ticket = self.next_ticket + 1;
            if !self.loader.try_submit(pos, ticket, self.ready.len()) {
                break;
            }
            self.next_ticket = ticket;
            self.in_flight.insert(pos, ticket);
        }
        loaded
    }

    /// [`Self::ensure`], then block until wanted chunks are resident or have
    /// failed closed (initial load and tests; never on the tick). Workers stay
    /// pipelined: finished loads are collected without waiting and each round
    /// admits the finished prefix of the wanted set in `(priority, position)`
    /// order, so stamps never depend on worker completion order. It waits for
    /// one load only when the next chunk in that order is still running.
    pub fn ensure_blocking(&mut self, want: &HashMap<ChunkPos, i32>) -> Vec<ChunkPos> {
        let mut loaded = Vec::new();
        loop {
            let round = self.admit_ready(want);
            let admitted = !round.is_empty();
            loaded.extend(round);
            if !want
                .keys()
                .any(|pos| !self.chunks.contains_key(pos) && !self.failed.contains_key(pos))
            {
                break;
            }
            if !admitted {
                assert!(
                    self.loader.pending() > 0,
                    "wanted chunks neither loading nor resident"
                );
                let finished = self.loader.recv();
                self.ready.push(finished);
            }
        }
        self.relight_admitted(&loaded, true);
        loaded
    }

    /// Make one loader result resident if its ticket is still the accepted
    /// one, the position is still wanted and not resident by another path.
    fn accept(
        &mut self,
        pos: ChunkPos,
        ticket: u64,
        result: Result<crate::chunk_loader::Produced, ChunkLoadError>,
        want: &HashMap<ChunkPos, i32>,
    ) -> bool {
        if self.in_flight.get(&pos) != Some(&ticket) {
            return false;
        }
        self.in_flight.remove(&pos);
        if !want.contains_key(&pos) || self.chunks.contains_key(&pos) {
            return false;
        }
        let (chunk, generated) = match result {
            Ok(chunk) => chunk,
            Err(error) => {
                self.record_load_error(error);
                return false;
            }
        };
        self.insert_produced(pos, chunk, generated)
    }

    fn record_load_error(&mut self, error: ChunkLoadError) {
        if let std::collections::hash_map::Entry::Vacant(entry) = self.failed.entry(error.pos) {
            tracing::error!(%error, "stored chunk unavailable; repair required");
            entry.insert(error);
        }
    }
    /// Quarantine invalid resident metadata without losing checkpoint custody.
    /// Clears a live `active` flag at this mutation boundary with the same
    /// frozen-boundary authority as [`Self::refresh_active_clock`]
    /// deactivation, so the single debt flag in [`Self::is_active`] never
    /// observes stale activity. Absent or already-inactive authority keeps
    /// the `None` absolute-reanchor fallback without erasing a boundary.
    pub(crate) fn reject_resident(&mut self, pos: ChunkPos, reason: String) {
        let was_resident = self.chunks.remove(&pos).is_some();
        self.recent.take(pos);
        self.in_flight.remove(&pos);
        self.revisions.remove(&pos);
        self.wire_revisions.remove(&pos);
        self.villager_route_revisions.remove(&pos);
        self.light.remove(pos);
        self.published = None;
        self.generated.retain(|captured| captured.pos != pos);
        self.dirty.remove(&pos);
        self.dirty_sections.remove(&pos);
        self.dirty_absent.insert(pos);
        if was_resident {
            self.active_revision += 1;
            self.poi_revisions.insert(pos);
            let tick = self.sim_tick;
            let boundary = match self.sim_debt.get_mut(&pos) {
                Some(debt) if debt.active => {
                    let boundary = (debt.last_sim, debt.remaining);
                    debt.active = false;
                    debt.dirty = true;
                    Some(boundary)
                }
                _ => None,
            };
            if let Some((last_sim, remaining)) = boundary {
                self.next_revision += 1;
                self.revisions.insert(pos, self.next_revision);
                self.eviction_check.insert(pos);
                self.dirty.insert(pos);
                let image = crate::journal::SimDebtImage {
                    chunk: [pos.x, pos.z],
                    last_sim,
                    remaining,
                    active: false,
                    tick,
                };
                self.activity_changes.insert(pos, Some(image));
                self.debt_changes.insert(pos, image);
            } else {
                self.activity_changes.entry(pos).or_insert(None);
            }
        }
        self.record_load_error(ChunkLoadError {
            dimension: self.source.dimension,
            pos,
            reason,
        });
    }

    /// Stored-load failures, including dimension and exact decoder reason.
    pub fn load_errors(&self) -> impl Iterator<Item = &ChunkLoadError> {
        self.failed.values()
    }

    pub fn load_error(&self, pos: ChunkPos) -> Option<&ChunkLoadError> {
        self.failed.get(&pos)
    }

    /// Make a loaded or generated chunk resident; queue its immutable
    /// generated base while the bounded save buffer has room.
    fn insert_produced(&mut self, pos: ChunkPos, chunk: Chunk, generated: bool) -> bool {
        let mut chunk = Arc::new(chunk);
        let generated_base = (generated
            && self.source.native.is_some()
            && self.generated.len() < GENERATED_PENDING_MAX)
            .then(|| Arc::clone(&chunk));
        if !self.apply_inhabited_image(pos, &mut chunk) {
            return false;
        }
        if let Some(base) = generated_base {
            self.generated.push(CapturedChunk {
                pos,
                revision: 0,
                chunk: base,
                dirty_sections: None,
            });
        }
        self.chunks.insert(pos, chunk);
        self.active_revision += 1;
        self.bump_revision(pos);
        true
    }

    /// A load may have read native storage before an absent counter capture
    /// committed. Its live interval remains authoritative across admission.
    fn apply_inhabited_image(&mut self, pos: ChunkPos, chunk: &mut Arc<Chunk>) -> bool {
        if let Err(error) = crate::debt::adopt_native_extras(pos, chunk, &mut self.sim_debt) {
            self.record_load_error(ChunkLoadError {
                dimension: self.source.dimension,
                pos,
                reason: error.to_string(),
            });
            return false;
        }
        if let Some(counter) = self.inhabited_time.get(&pos)
            && chunk.inhabited_time() != counter.image.value_at(self.inhabited_tick)
        {
            counter
                .image
                .apply(Arc::make_mut(chunk), self.inhabited_tick);
        }
        true
    }

    /// Generated immutable bases to save once at least `min` are queued.
    /// Native fill-generated checks coverage atomically, so a delayed batch
    /// never overwrites imported data or checkpointed gameplay.
    pub fn take_generated(&mut self, min: usize) -> Option<ChunkCapture> {
        if self.generated.is_empty() || self.generated.len() < min {
            return None;
        }
        Some(ChunkCapture {
            source: Arc::clone(&self.source),
            chunks: std::mem::take(&mut self.generated),
            deferred: HashMap::default(),
            load_error: None,
            generated_bases: true,
        })
    }

    /// Make `pos` resident now (checkpoint paths). Any off-tick load of the
    /// same chunk loses its ticket: its data was read before this
    /// checkpoint's native commit and must never land.
    fn materialize(&mut self, pos: ChunkPos) -> bool {
        if self.chunks.contains_key(&pos) {
            return true;
        }
        if self.failed.contains_key(&pos) {
            return false;
        }
        self.in_flight.remove(&pos);
        match self.recent.take(pos) {
            Some(mut chunk) => {
                if !self.apply_inhabited_image(pos, &mut chunk) {
                    return false;
                }
                self.chunks.insert(pos, chunk);
                self.active_revision += 1;
                self.bump_revision(pos);
            }
            None => match self.source.load_or_generate(pos) {
                Ok((chunk, generated)) => {
                    if !self.insert_produced(pos, chunk, generated) {
                        return false;
                    }
                }
                Err(error) => {
                    self.record_load_error(error);
                    return false;
                }
            },
        }
        self.relight_admitted(&[pos], false);
        true
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
    /// Read the live counter, including ticks not yet captured for persistence.
    pub(crate) fn inhabited_time(&self, pos: ChunkPos) -> i64 {
        self.inhabited_time.get(&pos).map_or_else(
            || self.chunk(pos).map_or(0, Chunk::inhabited_time).max(0),
            |counter| counter.image.value_at(self.inhabited_tick),
        )
    }

    /// Count a resident chunk once per game tick, regardless of player overlap.
    /// Unchanged active intervals neither allocate nor copy shared chunks.
    pub(crate) fn tick_inhabited_time(&mut self, pos: ChunkPos, tick: u64) {
        let Some(chunk) = self.chunks.get(&pos) else {
            return;
        };
        self.inhabited_tick = tick;
        let counter = self
            .inhabited_time
            .entry(pos)
            .or_insert_with(|| InhabitedCounter {
                image: crate::journal::InhabitedTimeImage {
                    chunk: [pos.x, pos.z],
                    value: chunk.inhabited_time().max(0),
                    tick,
                    active: false,
                },
                last_seen: tick.wrapping_sub(1),
                dirty: false,
            });
        if counter.last_seen == tick {
            return;
        }
        counter.last_seen = tick;
        self.inhabited_current.push(pos);
        if !counter.image.active {
            // Opening/closing an interval changes its stored value. Continuing
            // live time is already covered by the checkpoint's manifest anchor;
            // it must not force a full chunk re-encode every checkpoint.
            counter.dirty = true;
            counter.image.value = counter.image.value.saturating_add(1);
            counter.image.tick = tick;
            counter.image.active = true;
            self.inhabited_changes.push(counter.image);
        }
    }

    pub(crate) fn finish_inhabited_tick(&mut self, tick: u64) {
        self.inhabited_tick = tick;
        for pos in self.inhabited_active.drain(..) {
            let counter = self.inhabited_time.get_mut(&pos).expect("active interval");
            if counter.last_seen != tick {
                counter.image.value = counter.image.value_at(tick.saturating_sub(1));
                counter.image.tick = tick;
                counter.image.active = false;
                counter.dirty |= self
                    .chunks
                    .get(&pos)
                    .is_some_and(|chunk| chunk.inhabited_time() != counter.image.value);
                self.inhabited_changes.push(counter.image);
                self.eviction_check.insert(pos);
            }
        }
        std::mem::swap(&mut self.inhabited_active, &mut self.inhabited_current);
    }

    pub(crate) fn journal_inhabited_time(
        &mut self,
        entries: &mut Vec<crate::journal::JournalEntry>,
    ) {
        // Pending transitions can span more than one tick before capture.
        // Keep the latest absolute image and anchor it at this batch's clock.
        self.inhabited_changes
            .sort_unstable_by_key(|image| (image.chunk, std::cmp::Reverse(image.tick)));
        self.inhabited_changes.dedup_by_key(|image| image.chunk);
        entries.extend(self.inhabited_changes.drain(..).map(|image| {
            crate::journal::JournalEntry::InhabitedTime {
                image: crate::journal::InhabitedTimeImage {
                    value: image.value_at(self.inhabited_tick),
                    tick: self.inhabited_tick,
                    ..image
                },
            }
        }));
    }

    pub(crate) fn inhabited_checkpoint(
        &self,
        tick: u64,
    ) -> Vec<crate::journal::InhabitedTimeImage> {
        let mut images: Vec<_> = self
            .inhabited_active
            .iter()
            .map(|pos| {
                let image = self.inhabited_time[pos].image;
                crate::journal::InhabitedTimeImage {
                    value: image.value_at(tick),
                    tick,
                    ..image
                }
            })
            .collect();
        images.sort_unstable_by_key(|image| image.chunk);
        images
    }

    /// Restore durable checkpoint counters without making them dirty again.
    /// `None` denotes WAL-only/injected images without a persisted base.
    pub(crate) fn restore_inhabited_time(
        &mut self,
        replay: &crate::journal::Replay,
        checkpoint: Option<&crate::journal::CheckpointManifest>,
    ) {
        self.inhabited_tick = replay.tick;
        let persisted = checkpoint.map(|base| {
            (
                base.tick,
                base.inhabited_time
                    .iter()
                    .map(|image| (image.chunk, image))
                    .collect::<HashMap<_, _>>(),
            )
        });
        for (&[x, z], &image) in &replay.inhabited_time {
            let dirty = persisted.as_ref().is_none_or(|(tick, images)| {
                images
                    .get(&image.chunk)
                    .is_none_or(|base| image.value_at(replay.tick) != base.value_at(*tick))
            }) || image.active;
            // Recovery keeps an open interval, whose clean manifest value may be
            // newer than the native chunk after clock-only checkpoints. Pin it
            // dirty so closing it (or an absent chunk) persists the value.
            let pos = ChunkPos { x, z };
            let image = crate::journal::InhabitedTimeImage {
                value: image.value_at(replay.tick),
                tick: replay.tick,
                ..image
            };
            if image.active {
                self.inhabited_active.push(pos);
            }
            self.inhabited_time.insert(
                pos,
                InhabitedCounter {
                    image,
                    last_seen: replay.tick,
                    dirty,
                },
            );
        }
    }

    /// Chunks whose live clock changed admission during this batch. Residency
    /// alone and debt consumption do not invalidate unrelated entity anchors.
    pub(crate) fn simulation_clock_changes(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.activity_changes.keys().copied()
    }

    /// Drain pending debt post-images into the tick WAL batch (called at the
    /// END of `journal_entries_for_tick`). Latest image per chunk wins.
    pub(crate) fn journal_debt(&mut self, entries: &mut Vec<crate::journal::JournalEntry>) {
        let start = entries.len();
        entries.extend(self.debt_changes.drain().map(|(_, image)| {
            crate::journal::JournalEntry::SimDebt {
                image: crate::journal::SimDebtImage {
                    last_sim: image.last_sim_at(self.sim_tick),
                    tick: self.sim_tick,
                    ..image
                },
            }
        }));
        entries[start..].sort_unstable_by_key(|entry| match entry {
            crate::journal::JournalEntry::SimDebt { image } => image.chunk,
            _ => unreachable!("only debt images appended"),
        });
        self.activity_changes.clear();
    }

    /// Authority debt images for the checkpoint manifest at `tick`: dirty or
    /// nonzero-debt chunks. Mirrors `inhabited_checkpoint` (owned Vec, the
    /// manifest stores it; the journal thread never reads live records).
    pub(crate) fn debt_checkpoint(&self, tick: u64) -> Vec<crate::journal::SimDebtImage> {
        let mut images: Vec<_> = self
            .sim_debt
            .iter()
            .filter(|(_, debt)| debt.dirty || debt.remaining > 0 || debt.active)
            .map(|(pos, debt)| crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: debt.last_sim,
                remaining: debt.remaining,
                active: debt.active,
                tick,
            })
            .collect();
        images.sort_unstable_by_key(|image| image.chunk);
        images
    }

    /// Restore debt authority, distinguishing durable checkpoint images from
    /// newer WAL images. A consumed zero still pins its chunk until committed;
    /// an already durable inactive image does not need another chunk rewrite.
    /// `None` denotes WAL-only/injected images without a persisted base.
    pub(crate) fn restore_sim_debt(
        &mut self,
        replay: &crate::journal::Replay,
        checkpoint: Option<&crate::journal::CheckpointManifest>,
    ) -> Result<(), CoreError> {
        // Refuse unrepresentable WAL/manifest authority before activation can
        // consume it or any partial restore can mutate the live map.
        for (&[x, z], &image) in &replay.sim_debt {
            crate::debt::native_values(
                ChunkPos { x, z },
                image.last_sim_at(replay.tick),
                image.remaining,
            )?;
        }
        self.sim_tick = replay.tick;
        let persisted = checkpoint.map(|base| {
            (
                base.tick,
                base.debt
                    .iter()
                    .map(|image| (image.chunk, image))
                    .collect::<HashMap<_, _>>(),
            )
        });
        for (&[x, z], &image) in &replay.sim_debt {
            let pos = ChunkPos { x, z };
            let last_sim = image.last_sim_at(replay.tick);
            let dirty = persisted.as_ref().is_none_or(|(tick, images)| {
                images.get(&image.chunk).is_none_or(|base| {
                    last_sim != base.last_sim_at(*tick) || image.remaining != base.remaining
                })
            }) || image.active;
            // Recovery closes open debt intervals. A clean manifest anchor may
            // be newer than the native chunk after clock-only checkpoints, so
            // persist that closure before an inactive zero leaves the manifest.
            self.sim_debt.insert(
                pos,
                crate::debt::SimDebtRecord {
                    last_sim,
                    remaining: image.remaining,
                    dirty,
                    active: false,
                },
            );
            if dirty || image.active {
                self.debt_changes.insert(
                    pos,
                    crate::journal::SimDebtImage {
                        chunk: [x, z],
                        last_sim,
                        remaining: image.remaining,
                        active: false,
                        tick: replay.tick,
                    },
                );
            }
            if dirty {
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
            }
        }
        Ok(())
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

    /// Live clock bounds `(start, end]` since a lazy mirror's membership fence.
    /// Refresh mirrors in `simulation_clock_changes` before `journal_debt`
    /// drains the boundary images, so `from` never spans an older transition.
    /// Debt consumption is not an activation. Only countdowns use `end - start`.
    #[must_use]
    pub fn simulation_interval(&self, pos: ChunkPos, from: u64, to: u64) -> Option<(u64, u64)> {
        if to <= from {
            return None;
    /// Blocks also tick in resident level-32 neighbours of forced tickets.
    #[must_use]
    pub fn is_block_active(&self, cx: i32, cz: i32) -> bool {
        self.is_active(cx, cz)
            || (self.block_only.contains(&ChunkPos { x: cx, z: cz }) && self.has_chunk(cx, cz))
    }

        }
        let (start, end) = if let Some(Some(image)) = self.activity_changes.get(&pos) {
            if image.active {
                // Activation simulates the boundary tick itself.
                (from.max(image.tick.saturating_sub(1)), to)
            } else {
                // Deactivation freezes before this tick's simulation.
                (from, image.last_sim.min(to))
            }
        } else if self.is_active(pos.x, pos.z) {
            (from, to)
        } else {
            return None;
        };
        (start < end).then_some((start, end))
    }

    /// Parent-computed active set with hysteresis, called pre-gameplay, after
    /// inputs and after ensure at the same output tick (tick+1) idempotently.
    /// Transition-based (no same-tick skip): residency gates activation, so
    /// demand admitted later the same tick still consumes on that call.
    /// Only live-time accrual is tick-guarded (`tick > last_sim`).
    /// Activation records inactive elapsed time as debt, excluding this live
    /// tick. The merge owner consumes bounded whole-chunk steps separately.
    /// Continuing active advances only the live anchor, never remaining debt.
    pub fn set_active_chunks(&mut self, active: HashSet<ChunkPos>, tick: u64) {
        if self.active_set != active {
            self.active_revision += 1;
        }
        self.active_set = active;
        self.refresh_active_clock(tick);
    }

    pub(crate) fn set_block_only_chunks(&mut self, chunks: HashSet<ChunkPos>) {
        if self.block_only != chunks {
            self.active_revision += 1;
            self.block_only = chunks;
        }
    }

    pub(crate) fn entity_active_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.active_set.iter().copied()
    }

    /// Parent resident-union input: chunks pinning eviction (dirty, absent,
    /// debt or inhabited images). Duplicates possible; the caller dedups.
    pub fn dirty_pins(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.dirty
            .iter()
            .copied()
            .chain(self.dirty_absent.iter().copied())
            .chain(
                self.sim_debt
                    .iter()
                    .filter_map(|(pos, debt)| debt.dirty.then_some(*pos)),
            )
            .chain(
                self.inhabited_time
                    .iter()
                    .filter_map(|(pos, counter)| counter.dirty.then_some(*pos)),
            )
    }

    /// Reconcile resident/active transitions and advance `last_sim` at `tick`
    /// All ticking demand (resident or not), without duplicates.
    pub fn active_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.active_set.iter().chain(&self.block_only).copied()
    }

    /// without taking a membership set. Parent's cached-basis fast path:
    /// stationary ticks skip the set rebuild and call this after ensure so
    /// newly admitted chunks with active demand activate in the same tick.
    /// [`Self::set_active_chunks`] stores then calls this same method.
    pub fn refresh_active_clock(&mut self, tick: u64) {
        self.sim_tick = tick;
        // Hot path: no per-tick allocation or sorting. Disjoint field borrows
        // cover activation, accrual and their storage revisions inline.
        // `debt_changes` order is normalized at WAL drain (`journal_debt`).
        let Self {
            chunks,
            sim_debt,
            active_set,
            dirty,
            debt_changes,
            activity_changes,
            revisions,
            next_revision,
            eviction_check,
            active_revision,
            ..
        } = self;
        // Deactivations first: freeze records leaving membership.
        for (pos, debt) in sim_debt.iter_mut() {
            if debt.active && (!active_set.contains(pos) || !chunks.contains_key(pos)) {
                debt.active = false;
                *active_revision += 1;
                debt.dirty = true;
                *next_revision += 1;
                revisions.insert(*pos, *next_revision);
                eviction_check.insert(*pos);
                dirty.insert(*pos);
                let image = crate::journal::SimDebtImage {
                    chunk: [pos.x, pos.z],
                    last_sim: debt.last_sim,
                    remaining: debt.remaining,
                    active: false,
                    tick,
                };
                activity_changes.insert(*pos, Some(image));
                debt_changes.insert(*pos, image);
            }
        }
        // Activations + live accrual over stored membership. Residency gates
        // activation: demand admitted later the same tick still consumes on
        // this call; demand that loses residency deactivates and freezes.
        for pos in active_set.iter() {
            let resident = chunks.contains_key(pos);
            if !resident {
                if let Some(record) = sim_debt.get_mut(pos) {
                    record.active = false;
                }
                continue;
            }
            let record = sim_debt.entry(*pos).or_insert(crate::debt::SimDebtRecord {
                last_sim: tick,
                remaining: 0,
                dirty: false,
                active: false,
            });
            if record.active {
                if tick > record.last_sim {
                    record.last_sim = tick;
                }
                continue;
            }
            // Accrue only the inactive interval, never the activation's live tick.
            record.remaining = record
                .remaining
                .saturating_add(tick.saturating_sub(1).saturating_sub(record.last_sim));
            record.active = true;
            *active_revision += 1;
            record.last_sim = tick;
            record.dirty = true;
            // Storage-only fence inline (never wire/published/hydration: this
            // is metadata authority, not client-visible content).
            *next_revision += 1;
            revisions.insert(*pos, *next_revision);
            eviction_check.insert(*pos);
            dirty.insert(*pos);
            let image = crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: tick,
                remaining: record.remaining,
                active: true,
                tick,
            };
            activity_changes.insert(*pos, Some(image));
            debt_changes.insert(*pos, image);
        }
    }

    pub(crate) fn simulation_debt(&self, pos: ChunkPos) -> u64 {
        self.sim_debt.get(&pos).map_or(0, |debt| debt.remaining)
    }

    /// Resident-only worklist: dormant work never requests chunk admission.
    pub(crate) fn catch_up_chunks(&self) -> Vec<ChunkPos> {
        let mut chunks: Vec<_> = self
            .sim_debt
            .iter()
            .filter_map(|(&pos, debt)| {
                (debt.active && debt.remaining > 0 && self.chunks.contains_key(&pos)).then_some(pos)
            })
            .collect();
        chunks.sort_unstable_by_key(|pos| (pos.x, pos.z));
        chunks
    }

    /// Debt and effects are captured by the same tick WAL/checkpoint fence.
        // Level 32 runs block time live, not entity time. Do not later replay
        // those same ticks as whole-chunk catch-up when this chunk reaches 31.
        // These anchors are rebuilt from persisted ticket demand on recovery.
        for pos in &self.block_only {
            if !chunks.contains_key(pos) {
                continue;
            }
            let record = sim_debt.entry(*pos).or_insert(crate::debt::SimDebtRecord {
                last_sim: tick,
                remaining: 0,
                dirty: false,
                active: false,
            });
            record.last_sim = tick;
            record.remaining = 0;
        }
    pub(crate) fn consume_simulation_debt(&mut self, pos: ChunkPos, elapsed: u64) {
        let Some(debt) = self.sim_debt.get_mut(&pos) else {
            return;
        };
        debt.remaining = debt.remaining.saturating_sub(elapsed);
        debt.dirty = true;
        self.debt_changes.insert(
            pos,
            crate::journal::SimDebtImage {
                chunk: [pos.x, pos.z],
                last_sim: debt.last_sim,
                remaining: debt.remaining,
                active: debt.active,
                tick: self.sim_tick,
            },
        );
        self.dirty.insert(pos);
        // Metadata must not request rehydration or invalidate client light.
        self.next_revision += 1;
        self.revisions.insert(pos, self.next_revision);
        self.eviction_check.insert(pos);
    }

    /// 001-f: an accepted external write invalidates old time, including an
    /// inactive interval not yet materialized as remaining debt.
    pub(crate) fn cancel_simulation_debt(&mut self, pos: ChunkPos) {
        let Some(debt) = self.sim_debt.get_mut(&pos) else {
            return;
        };
        if debt.remaining == 0 && (debt.active || debt.last_sim >= self.sim_tick) {
            return;
        }
        debt.last_sim = self.sim_tick;
        self.consume_simulation_debt(pos, u64::MAX);
    }

    /// Retire acknowledged empty queue coverage only when no old native image
    /// can still become resident. Worker tickets and bounded recent-cache
    /// copies may predate the checkpoint commit even after content is clean.
    pub(crate) fn unloaded_clean(&self, x: i32, z: i32) -> bool {
        let pos = ChunkPos { x, z };
        !self.chunks.contains_key(&pos)
            && !self.in_flight.contains_key(&pos)
            && !self.recent.chunks.contains_key(&pos)
            && !self.dirty.contains(&pos)
            && !self.dirty_absent.contains(&pos)
            && !self
                .overlay
                .keys()
                .any(|(bx, _, bz)| bx.div_euclid(16) == x && bz.div_euclid(16) == z)
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

    /// Inject checkpoint block-entity storage into a chunk's typed maps
    /// (cleared first for this chunk's columns so removals persist).
    fn inject_block_entities(
        chunk: &mut mc_world::Chunk,
        pos: ChunkPos,
        block_entities: &crate::core::CheckpointBlockEntities,
        registry: &BlockRegistry,
    ) {
        chunk.chests.retain(|block, chest| {
            !Self::in_chunk(*block, pos)
                || (chest.loot_table.is_some()
                    && !block_entities.contains_key(&(block.x, block.y, block.z)))
        });
        chunk
            .furnaces
            .retain(|block, _| !Self::in_chunk(*block, pos));
        chunk
            .hoppers
            .retain(|block, _| !Self::in_chunk(*block, pos));
        chunk
            .brewing_stands
            .retain(|block, _| !Self::in_chunk(*block, pos));
        for ((x, y, z), record) in block_entities {
            if !Self::in_chunk(
                mc_world::BlockPos {
                    x: *x,
                    y: *y,
                    z: *z,
                },
                pos,
            ) {
                continue;
            }
            let block = mc_world::BlockPos {
                x: *x,
                y: *y,
                z: *z,
            };
            // Native opaque owners (including three-slot shelves and nine-slot
            // crafters) must never be retyped according to inventory size.
            use mc_data::block_entity_kinds::BlockEntityKind;
            let kind = chunk
                .get_block(x.rem_euclid(16) as u8, *y, z.rem_euclid(16) as u8)
                .and_then(|id| registry.by_id(id))
                .and_then(|state| BlockEntityKind::for_block(state.block.id.path()));
            if matches!(
                kind,
                Some(
                    BlockEntityKind::Furnace
                        | BlockEntityKind::Smoker
                        | BlockEntityKind::BlastFurnace
                )
            ) {
                chunk.furnaces.insert(
                    block,
                    mc_world::FurnaceBlockEntity {
                        slots: Self::furnace_slots(&record.slots),
                        burn_remaining: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.lit_remaining as i16)
                            .unwrap_or(0),
                        burn_total: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.lit_total as i16)
                            .unwrap_or(0),
                        cook_progress: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.cook_progress as i16)
                            .unwrap_or(0),
                        cook_total: record
                            .furnace
                            .as_ref()
                            .map(|furnace| furnace.cook_total as i16)
                            .unwrap_or(200),
                        recipes_used: record
                            .furnace
                            .as_ref()
                            .map(|furnace| (*furnace.recipes_used).clone())
                            .unwrap_or_default(),
                    },
                );
            } else if matches!(
                kind,
                Some(
                    BlockEntityKind::Chest
                        | BlockEntityKind::TrappedChest
                        | BlockEntityKind::Barrel
                )
            ) {
                let mut slots: [mc_world::FurnaceSlot; 27] =
                    std::array::from_fn(|_| mc_world::FurnaceSlot::default());
                for (index, stack) in record.slots.iter().take(27).enumerate() {
                    slots[index] = mc_world::FurnaceSlot {
                        count: stack.count,
                        item_id: stack.item_id,
                        damage: stack.damage,
                        enchantments: stack.enchantments.clone(),
                        custom_name: stack.custom_name.as_deref().map(str::to_owned),
                        item_model: stack.item_model.clone(),
                        stew_effects: stack.stew_effects.clone(),
                        components: stack.components.clone(),
                    };
                }
                chunk.chests.insert(
                    block,
                    mc_world::ChestBlockEntity {
                        slots,
                        loot_table: None,
                        loot_seed: 0,
                    },
                );
            }
        }
    }

    fn in_chunk(block: mc_world::BlockPos, chunk: ChunkPos) -> bool {
        block.x.div_euclid(16) == chunk.x && block.z.div_euclid(16) == chunk.z
    }

    fn furnace_slots(slots: &[mc_data::ItemStack]) -> [mc_world::FurnaceSlot; 3] {
        let mut out: [mc_world::FurnaceSlot; 3] =
            std::array::from_fn(|_| mc_world::FurnaceSlot::default());
        for (index, stack) in slots.iter().take(3).enumerate() {
            out[index] = mc_world::FurnaceSlot {
                count: stack.count,
                item_id: stack.item_id,
                damage: stack.damage,
                enchantments: stack.enchantments.clone(),
                custom_name: stack.custom_name.as_deref().map(str::to_owned),
                item_model: stack.item_model.clone(),
                stew_effects: stack.stew_effects.clone(),
                components: stack.components.clone(),
            };
        }
        out
    }

    /// Capture the chunks a checkpoint must write, on the tick thread.
    /// Block-entity storage is injected into the chunks' typed maps first,
    /// so native chunks retain their full `block_entities`; pending absent
    /// chunks load-native-or-generate first so a container change is never
    /// lost to eviction. The capture shares the chunks by `Arc` (later
    /// edits copy on write) and leaves them dirty until [`Self::mark_flushed`].
    /// Absent inhabited/scheduler images are frozen metadata only; the journal
    /// reads their effective chunk and prepares derived light after the WAL gate.
    pub fn capture_dirty(
        &mut self,
        block_entities: &crate::core::CheckpointBlockEntities,
    ) -> ChunkCapture {
        let capture_start = std::time::Instant::now();
        let mut capture = ChunkCapture {
            source: Arc::clone(&self.source),
            chunks: Vec::new(),
            deferred: HashMap::default(),
            load_error: None,
            generated_bases: false,
        };
        // Fold resident intervals only at checkpoint; absent images are frozen
        // below without loading or admitting their chunks. WAL stores boundaries.
        let inhabited: Vec<_> = self
            .inhabited_time
            .iter()
            .filter_map(|(pos, counter)| counter.dirty.then_some(*pos))
            .collect();
        for pos in inhabited {
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                self.inhabited_time[&pos]
                    .image
                    .apply(Arc::make_mut(chunk), self.inhabited_tick);
            }
            let counter = self.inhabited_time.get_mut(&pos).unwrap();
            counter.dirty = false;
            self.dirty.insert(pos);
            self.bump_storage_revision(pos);
        }
        let inhabited_time = capture_start.elapsed();
        if capture.source.native.is_none() {
            return capture;
        }
        // Block writes whose chunk is not resident (journal replay before the
        // chunk loaded, or a chunk still loading off-tick) live only in the
        // overlay. This checkpoint supersedes their journal records, so make
        // those chunks resident and fold the writes before encoding.
        let homeless: HashSet<ChunkPos> = self
            .overlay
            .keys()
            .map(|(x, _, z)| ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            })
            .filter(|pos| !self.chunks.contains_key(pos))
            .collect();
        for pos in homeless {
            if !self.materialize(pos) {
                capture.load_error = self.failed.get(&pos).cloned();
            }
        }
        self.fold_overlay();
        // Absent chunks with pending block-entity changes: make them
        // resident through load-or-generate so the same write path persists
        // them (their block base comes from disk or worldgen).
        for pos in std::mem::take(&mut self.dirty_absent) {
            if self.materialize(pos) {
                self.dirty.insert(pos);
            } else {
                capture.load_error = self.failed.get(&pos).cloned();
                self.dirty_absent.insert(pos);
            }
        }
        let materialize_time = capture_start.elapsed();
        // Changed debt rides the chunk checkpoint. Clean live anchors and
        // unchanged nonzero debt already ride the manifest; advancing live
        // time must not rewrite every active chunk on every save.
        // Projection is durable-only and never changes the wire revision.
        let debt: Vec<ChunkPos> = self
            .sim_debt
            .iter()
            .filter_map(|(pos, debt)| debt.dirty.then_some(*pos))
            .collect();
        for pos in debt {
            if !self.chunks.contains_key(&pos) {
                // Non-resident debt rides the deferred path (frozen image,
                // journal-thread application, revision-fenced ACK) — never
                // dropped pre-ACK.
                let (last_sim, remaining) = self
                    .sim_debt
                    .get(&pos)
                    .map(|debt| (debt.last_sim, debt.remaining))
                    .unwrap_or((0, 0));
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
                if let Some(deferred) = self.defer_checkpoint_chunk(pos, &mut capture) {
                    deferred.debt = Some(crate::journal::SimDebtImage {
                        chunk: [pos.x, pos.z],
                        last_sim,
                        remaining,
                        active: false,
                        tick: self.inhabited_tick,
                    });
                }
                continue;
            }
            if let Some(chunk) = self.chunks.get_mut(&pos) {
                let (last_sim, remaining) = self
                    .sim_debt
                    .get(&pos)
                    .map(|debt| (debt.last_sim, debt.remaining))
                    .unwrap_or((0, 0));
                if let Err(error) = crate::debt::project(Arc::make_mut(chunk), last_sim, remaining)
                {
                    capture.load_error = Some(ChunkLoadError {
                        dimension: self.source.dimension,
                        pos,
                        reason: error.to_string(),
                    });
                    self.dirty.insert(pos);
                    continue;
                }
                // Projection changes the stored image: keep the fence until ACK.
                self.dirty.insert(pos);
                self.bump_storage_revision(pos);
            }
            if let Some(debt) = self.sim_debt.get_mut(&pos) {
                debt.dirty = false;
            }
        }
        let dirty: Vec<ChunkPos> = self.dirty.iter().copied().collect();
        for pos in dirty {
            if !self.chunks.contains_key(&pos) {
                if let Some(counter) = self.inhabited_time.get(&pos) {
                    let image = crate::journal::InhabitedTimeImage {
                        value: counter.image.value_at(self.inhabited_tick),
                        tick: self.inhabited_tick,
                        active: false,
                        ..counter.image
                    };
                    if let Some(deferred) = self.defer_checkpoint_chunk(pos, &mut capture) {
                        deferred.inhabited = Some(image);
                    }
                }
                continue;
            }
            let chunk = self.chunks.get_mut(&pos).unwrap();
            if let Some(counter) = self.inhabited_time.get(&pos)
                && chunk.inhabited_time() != counter.image.value_at(self.inhabited_tick)
            {
                counter
                    .image
                    .apply(Arc::make_mut(chunk), self.inhabited_tick);
            }
            // Typed inventory/progress projection is durable-only. It must
            // not invalidate a client's already delivered chunk image.
            let has_block_entities = !chunk.chests.is_empty()
                || !chunk.furnaces.is_empty()
                || block_entities.keys().any(|(x, y, z)| {
                    Self::in_chunk(
                        mc_world::BlockPos {
                            x: *x,
                            y: *y,
                            z: *z,
                        },
                        pos,
                    )
                });
            if has_block_entities {
                Self::inject_block_entities(
                    Arc::make_mut(chunk),
                    pos,
                    block_entities,
                    &self.source.registry,
                );
                self.bump_storage_revision(pos);
            }
            capture.chunks.push(self.captured_chunk(pos));
        }
        if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
            eprintln!(
                "checkpoint chunks ms: inhabited={:.2} materialize={:.2} project={:.2} chunks={}",
                inhabited_time.as_secs_f64() * 1000.0,
                (materialize_time - inhabited_time).as_secs_f64() * 1000.0,
                (capture_start.elapsed() - materialize_time).as_secs_f64() * 1000.0,
                capture.chunks.len() + capture.deferred.len(),
            );
        }
        capture
    }

    /// A checkpoint wrote these `(position, revision)`s: chunks still at
    /// that revision are clean; newer edits stay dirty for the next one.
    pub fn mark_flushed(&mut self, written: &[(ChunkPos, u64)]) {
        for (pos, revision) in written {
            if self.revisions.get(pos) == Some(revision) {
                self.dirty.remove(pos);
                self.dirty_sections.remove(pos);
                if let Some(debt) = self.sim_debt.get_mut(pos) {
                    debt.dirty = false;
                }
                self.eviction_check.insert(*pos);
            }
        }
    }

    /// Frozen image of a resident chunk for the next checkpoint. Edits after
    /// this call copy on write and bump the revision, so the ACK fence keeps
    /// them (and this mask's bits) for the next capture.
    fn captured_chunk(&self, pos: ChunkPos) -> CapturedChunk {
        CapturedChunk {
            pos,
            revision: self.revisions.get(&pos).copied().unwrap_or(0),
            chunk: Arc::clone(&self.chunks[&pos]),
            dirty_sections: Some(self.dirty_sections.get(&pos).copied().unwrap_or(0)),
        }
    }

    /// Capture, write and acknowledge in one call (tests).
    #[cfg(test)]
    pub fn flush_dirty(
        &mut self,
        block_entities: &crate::core::CheckpointBlockEntities,
    ) -> Result<usize, CoreError> {
        let capture = self.capture_dirty(block_entities);
        let written = capture.write()?;
        self.mark_flushed(&written);
        Ok(written.len())
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
