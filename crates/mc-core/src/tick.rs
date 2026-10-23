//! Tick driver: inputs -> entity phase -> merge -> block phase ->
//! commit -> publish, at 20 TPS (ADR 0013 "Tick").
//!
//! The entity phase runs in parallel by data over scoped threads sized once
//! from the machine (no work stealing across phase boundaries, no locks on
//! the tick path: workers return owned writes). Merge applies intents one at
//! a time in fixed order with whole-intent rejection. The block phase runs
//! serially in vanilla-like update order. Commit folds blocks, applies entity
//! writes, and publishes the snapshot. Publish hands a [`TickOutput`] to broadcast
//! (world state: laggards skip to latest) and per-player
//! [`PlayerChange`](crate::boundary::PlayerChange) results to bounded
//! per-connection queues (never skipped; overflow disconnects).

use mc_physics::mth;
use std::collections::{BTreeMap, BTreeSet};

use mc_data::statistics::StatType;
use rustc_hash::{FxHashMap as HashMap, FxHashSet as HashSet};
use std::sync::Arc;

use crate::boundary::{
    BlockPos, ChangeSet, ChunkPos, ConnectionId, DigAction, EntityAppearance, EntityEvent,
    EntityId, Hand, PlayerChange, PlayerInput, TickOutput, TickTimes, WorldSnapshot,
};
use crate::entities::{
    Body, ColdFields, EntityKind, EntitySlot, EntityStore, MOTION_ALIVE, MOTION_EFFECTS,
    MOTION_OWNED, MOTION_PHASE_MASK, MOTION_PHASE_SHIFT, MOTION_PLAYER, MOTION_REST,
    MOTION_SPECIAL,
};
use crate::intent::{Footprint, Intent, IntentKind, InvClaim, sort_intents};
use crate::registry::CoreRegistry;
use crate::world::ChunkWorld;

mod auth;
pub(crate) use auth::unlock_session;
mod damage;
mod elytra;
mod input;
pub(crate) mod merchant;
pub(crate) mod mining;
mod stations;

#[cfg(test)]
pub(crate) mod mining_tests;

#[cfg(test)]
#[path = "entity_phase_tests.rs"]
mod entity_phase_tests;

#[cfg(test)]
#[path = "entity_prephase_tests.rs"]
mod entity_prephase_tests;

#[cfg(test)]
#[path = "auth_gate_tests.rs"]
mod auth_gate_tests;

/// Fixed worker count: derived once from the machine.
#[must_use]
pub fn worker_threads() -> usize {
    std::thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(4)
        .max(1)
}

/// Reuse capacity, not a slot snapshot: age expiry can compact the store before
/// despawn, and the two callers have different global-player admission rules.
pub(crate) fn active_entity_slots(
    state: &mut TickState,
    include_global_players: bool,
) -> Vec<EntitySlot> {
    let mut slots = std::mem::take(&mut state.serial_entity_slots);
    slots.clear();
    // Every row has current chunk membership. When all occupied chunks are
    // active, the dense range is already the unique, ordered admission list.
    let all_active = state
        .entities
        .villager_chunks()
        .keys()
        .all(|chunk| state.world.is_active(chunk.x, chunk.z))
        && state
            .entities
            .brain_sensor_chunks()
            .keys()
            .all(|chunk| state.world.is_active(chunk.x, chunk.z));
    if all_active {
        slots.extend(state.entities.slots());
        return slots;
    }
    // Exact membership permits a dense scan when most rows are active, even
    // with inactive occupied chunks. Keep sparse activity and old transition
    // entries on the indexed union below.
    if prefer_dense_entity_admission(state) {
        let mut previous: Option<(ChunkPos, bool)> = None;
        for slot in state.entities.slots() {
            let chunk = state.entities.chunk_of(slot);
            let active = match previous {
                Some((last, active)) if last == chunk => active,
                _ => {
                    let active = state.world.is_active_position(chunk.x, chunk.z);
                    previous = Some((chunk, active));
                    active
                }
            };
            if active
                || include_global_players
                    && state.players.contains_key(&state.entities.body(slot).id)
            {
                slots.push(slot);
            }
        }
        return slots;
    }
    let mut marks = std::mem::take(&mut state.serial_entity_marks);
    marks.resize(state.entities.len(), 0);
    let mut admitted = 0;
    for chunk in state
        .world
        .active_positions()
        .filter(|chunk| state.world.is_active(chunk.x, chunk.z))
    {
        state.entities.for_each_slot_in_chunk(chunk, |slot| {
            let mark = &mut marks[slot.0 as usize];
            admitted += usize::from(*mark == 0);
            *mark = 1;
        });
    }
    if include_global_players {
        for id in state.players.keys() {
            if let Some(slot) = state.entities.slot_of_any(*id) {
                let mark = &mut marks[slot.0 as usize];
                admitted += usize::from(*mark == 0);
                *mark = 1;
            }
        }
    }
    slots.reserve(admitted);
    for (index, &marked) in marks.iter().enumerate() {
        if marked != 0 {
            slots.push(EntitySlot(index as u32));
        }
    }
    marks.clear();
    state.serial_entity_marks = marks;
    slots
}

/// Read hot positions only for mostly active populations with exact membership.
/// Bucket counts reject widely dormant worlds before any per-chunk lookups;
/// population weights also reject dormant rows concentrated in a few chunks.
fn prefer_dense_entity_admission(state: &TickState) -> bool {
    let occupied =
        state.entities.villager_chunks().len() + state.entities.brain_sensor_chunks().len();
    // Each occupied chunk can occur in both indexes. Conservatively retain the
    // indexed path when active chunks cannot cover half the bucket entries.
    if occupied > state.world.active_positions().count().saturating_mul(4)
        || !state.entities.has_exact_chunk_membership()
    {
        return false;
    }
    let needed = state.entities.len().div_ceil(2);
    let mut active = 0;
    for (chunk, ids) in state
        .entities
        .villager_chunks()
        .iter()
        .chain(state.entities.brain_sensor_chunks())
    {
        if state.world.is_active_position(chunk.x, chunk.z) {
            active += ids.len();
            if active >= needed {
                return true;
            }
        }
    }
    active >= needed
}

/// Connections registry: connection -> player entity + identity.
#[derive(Debug, Default)]
pub struct Connections {
    by_connection: HashMap<ConnectionId, EntityId>,
    by_entity: HashMap<EntityId, ConnectionId>,
    names: HashMap<ConnectionId, String>,
    uuids: HashMap<ConnectionId, uuid::Uuid>,
    /// Transient negotiated client view radii, keyed by live connection.
    /// Recorded on [`PlayerInput::SetViewDistance`](crate::boundary::PlayerInput::SetViewDistance)
    /// (which precedes Join), moved with the connection across dimensions,
    /// and cleared on leave/remove. Never persisted: unnegotiated callers
    /// fall back to the configured server view.
    view_distances: HashMap<ConnectionId, u8>,
    /// Transient client UI locales, keyed by live connection.
    /// Recorded on [`PlayerInput::SetLocale`](crate::boundary::PlayerInput::SetLocale)
    /// (which precedes Join), moved with the connection across dimensions,
    /// and cleared on leave/remove. Never persisted: unnegotiated callers read
    /// English.
    locales: HashMap<ConnectionId, String>,
}

impl Connections {
    pub(crate) fn insert(
        &mut self,
        connection: ConnectionId,
        entity: EntityId,
        uuid: uuid::Uuid,
        name: String,
    ) {
        self.by_connection.insert(connection, entity);
        self.by_entity.insert(entity, connection);
        self.names.insert(connection, name);
        self.uuids.insert(connection, uuid);
    }

    /// Connection for a player entity, if joined.
    #[must_use]
    pub fn entity_connection(&self, entity: EntityId) -> Option<ConnectionId> {
        self.by_entity.get(&entity).copied()
    }

    /// Identity uuid for a connection, if joined.
    #[must_use]
    pub fn uuid_of(&self, connection: ConnectionId) -> Option<uuid::Uuid> {
        self.uuids.get(&connection).copied()
    }

    /// Display name for a player entity, if joined.
    #[must_use]
    pub fn name_of(&self, entity: EntityId) -> Option<&str> {
        self.by_entity
            .get(&entity)
            .and_then(|connection| self.names.get(connection))
            .map(String::as_str)
    }

    /// Negotiated client view radius for a connection, if the edge admitted one.
    /// Unnegotiated callers get `None` and fall back to the configured view.
    #[must_use]
    pub fn view_distance_of(&self, connection: ConnectionId) -> Option<u8> {
        self.view_distances.get(&connection).copied()
    }

    pub(crate) fn set_view_distance(&mut self, connection: ConnectionId, view_distance: u8) {
        self.view_distances.insert(connection, view_distance);
    }

    /// Client UI locale (`ClientInformation.language`) for a connection, if the
    /// edge admitted one. Unnegotiated callers get `None` and read English.
    #[must_use]
    pub fn locale_of(&self, connection: ConnectionId) -> Option<&str> {
        self.locales.get(&connection).map(String::as_str)
    }

    pub(crate) fn set_locale(&mut self, connection: ConnectionId, locale: String) {
        self.locales.insert(connection, locale);
    }

    pub(crate) fn remove_by_connection(&mut self, connection: ConnectionId) -> Option<EntityId> {
        self.view_distances.remove(&connection);
        self.locales.remove(&connection);
        let entity = self.by_connection.remove(&connection)?;
        self.by_entity.remove(&entity);
        self.names.remove(&connection);
        self.uuids.remove(&connection);
        Some(entity)
    }

    pub(crate) fn transfer_to(&mut self, connection: ConnectionId, destination: &mut Self) {
        let entity = self
            .by_connection
            .remove(&connection)
            .expect("joined connection");
        self.by_entity.remove(&entity);
        let uuid = self.uuids.remove(&connection).expect("joined UUID");
        let name = self.names.remove(&connection).expect("joined name");
        destination.insert(connection, entity, uuid, name);
        if let Some(view_distance) = self.view_distances.remove(&connection) {
            destination.view_distances.insert(connection, view_distance);
        }
        if let Some(locale) = self.locales.remove(&connection) {
            destination.locales.insert(connection, locale);
        }
    }

    /// Player entity for a connection, if joined.
    #[must_use]
    pub fn entity_of(&self, connection: ConnectionId) -> Option<EntityId> {
        self.by_connection.get(&connection).copied()
    }

    /// All joined connections (reliable fan-out targets).
    pub fn all_connections(&self) -> impl Iterator<Item = ConnectionId> + '_ {
        self.by_connection.keys().copied()
    }
}

/// One accepted damage intent, retaining its pre-commit hit context.
#[derive(Debug, Clone, Copy)]
pub struct DamagePiece {
    pub amount: f32,
    pub kind: crate::survival::DamageKind,
    pub source: Option<crate::intent::DamageSource>,
    pub(crate) physical: Option<crate::physical_defense::PhysicalImpact>,
    pub(crate) custom_armor: bool,
}

/// Authoritative tick state. Single owner: the tick thread.
pub struct TickState {
    pub config_sim_distance: i32,
    /// Client residency radius, independent of gameplay simulation.
    pub config_view_distance: i32,
    active_player_centers: Vec<ChunkPos>,
    active_ticket_ranges: Vec<(ChunkPos, i32)>,
    active_sim_radius: i32,
    pub registry: Arc<CoreRegistry>,
    pub world: ChunkWorld,
    /// This world's dimension; other worlds share its tick owner and WAL.
    pub dimension: crate::dimensions::Dimension,
    pub other_dimensions: std::collections::BTreeMap<crate::dimensions::Dimension, Box<TickState>>,
    pub(crate) dimension_requests: Vec<crate::dimensions::travel::Request>,
    pub(crate) dimension_respawns: Vec<EntityId>,
    pub(crate) dimension_routes: Vec<crate::dimensions::Dimension>,
    pub entities: EntityStore,
    pub connections: Connections,
    /// Opt-in native login fence; authorization belongs to the live session.
    pub(crate) require_plugin_login: bool,
    pub(crate) locked_sessions: BTreeSet<ConnectionId>,
    /// Consecutive unsupported non-descending movement samples permitted before
    /// anti-flight corrects and disconnects the player; zero disables it.
    pub(crate) max_airtime_ticks: u32,
    /// Per-mob switches layered under the `mob_griefing` gamerule.
    pub(crate) mob_griefing: crate::config::MobGriefingRules,
    /// Player state per player entity (inventory, menus, vitals, mode).
    pub players: HashMap<EntityId, crate::player::PlayerState>,
    /// Recovered offline records by uuid: consumed by Join to restore full
    /// player state (inventory, vitals, xp, mode, position).
    pub offline_players: HashMap<uuid::Uuid, crate::journal::ReplayPlayer>,
    pub(crate) advancement_events: Vec<crate::advancements::Event>,
    pub(crate) advancement_reward_spawns: Vec<EntityId>,
    pub(crate) native_globals:
        std::collections::BTreeMap<String, mc_world::native::NativeDomainRecord>,
    pub(crate) plugins: crate::plugin::PluginState,
    pub(crate) plugin_runtime: Option<crate::plugin::PluginRuntime>,
    pub(crate) build_queries: Option<crate::build_precommit::PreviewQueries>,
    pub(crate) plugin_spawned: Vec<EntityId>,
    pub(crate) plugin_block_writes: Vec<(BlockPos, u32)>,
    pub(crate) plugin_players: crate::plugin_player::PlayerAuthority,
    pub(crate) plugin_damage: Vec<(EntitySlot, DamagePiece)>,
    pub(crate) plugin_interactions: HashMap<(EntityId, u32), mc_script::ScriptEvent>,
    /// Per-connection reliable outbox (bounded; overflow disconnects).
    pub outbox: HashMap<ConnectionId, Vec<PlayerChange>>,
    pub outbox_cap: usize,
    /// Connections to drop after this tick's publish: (connection, tick the
    /// `Disconnect` was sealed into its queue).
    pub disconnects: Vec<(ConnectionId, u64)>,
    pub tick: u64,
    /// Unix seconds source for real-calendar stamps (advancement obtained
    /// dates). Production reads the system clock; deterministic runs inject a
    /// fixed one, so the wall clock is an explicit external input.
    pub(crate) wall_clock: fn() -> i64,
    /// Secret per-world salt of [`TickState::entity_uuid`]; the world's own is
    /// installed by `recover_tick_state`. The zero default is for unit-test
    /// states, which want reproducible ids.
    pub(crate) entity_salt: [u8; crate::entity_uuid::SALT_LEN],
    pub day_time: i64,
    /// Block edits staged by merge this tick: (pos, new state).
    pub merge_block_writes: Vec<(BlockPos, u32)>,
    /// Reused block-phase projection; merge writes remain visible to commit.
    block_phase_writes: Vec<(BlockPos, u32)>,
    /// Reused ordered admission for serial simulation passes.
    pub(crate) serial_entity_slots: Vec<EntitySlot>,
    /// Byte admission scratch, rebuilt for each caller after structural changes.
    pub(crate) serial_entity_marks: Vec<u8>,
    /// Reused predicate caches; each owning pass resets them before reading.
    pub(crate) contact_hazards_scratch: crate::contact_hazards::Scratch,
    pub(crate) portal_match_cache: crate::world::SectionMatchCache,
    /// Negative portal probes, checked against identity/cells/content revision.
    pub(crate) portal_contact_stamps: Vec<Option<crate::world::ContactStamp>>,
    /// Accepted breaks retain their once-rolled, pre-write loot image.
    pub merge_broken: Vec<(EntityId, BlockPos, u32, Vec<mc_data::ItemStack>, i32)>,
    /// Command destroys use empty-tool loot regardless of sender game mode.
    pub(crate) merge_command_broken: Vec<(BlockPos, u32, Vec<mc_data::ItemStack>)>,
    /// Container contents staged by merge for broken containers:
    /// ((x,y,z), slots). Commit spawns each non-empty slot as a drop,
    /// closes viewers, removes storage, marks dirty, stages the tombstone.
    pub merge_container_contents: Vec<((i32, i32, i32), Vec<mc_data::ItemStack>)>,
    /// Viewers of containers broken this tick: (viewer, broken cell). A
    /// double chest's viewers close when either half breaks, so they are
    /// found at merge time, while both halves still stand. Commit closes
    /// each menu.
    pub merge_closed_viewers: Vec<(EntityId, (i32, i32, i32))>,
    /// Entity damage staged by merge: target slot and source-aware hit.
    pub merge_damage: Vec<(EntitySlot, DamagePiece)>,
    /// Item entities consumed by merge.
    pub merge_consumed_items: HashSet<EntitySlot>,
    /// Children spawned by merge.
    pub merge_spawns: Vec<MergeSpawn>,
    /// Animal harvest drops, journaled with all other committed spawns.
    pub merge_life_drops: Vec<([f64; 3], mc_data::ItemStack)>,
    /// Buckets filled/emptied out of a stack: (actor, new bucket item).
    pub merge_gives: Vec<(EntityId, u32)>,
    /// Scheduled water/lava ticks.
    pub fluid_ticks: crate::fluids::FluidTicks,
    pub(crate) block_ticks: crate::block_ticks::BlockTicks,
    pub(crate) redstone: crate::redstone::RedstoneState,
    pub(crate) redstone_containers: crate::redstone::containers::ContainerState,
    /// Random-tick section eligibility keyed by committed chunk revision.
    pub(crate) farm_sections: crate::farming::FarmSections,
    /// Pickup events staged by merge: (item slot, collector slot, count).
    pub merge_pickups: Vec<(EntitySlot, EntitySlot, i32)>,
    /// Entities whose viewer-visible appearance changed this tick (mob
    /// health, partially picked item stacks): published as metadata.
    pub appearance_changed: Vec<EntityId>,
    /// Players whose final Leave image entered `offline_players` this tick;
    /// drained by the journal so it never scans every offline record.
    pub(crate) left_players: Vec<uuid::Uuid>,
    /// Reused tick-local fence index: knot id and whether a live leash uses it.
    pub(crate) leash_knots: HashMap<[i32; 3], (EntityId, bool)>,
    /// Remote tickets: owner -> (centre chunk, radius).
    pub tickets: BTreeMap<String, (ChunkPos, i32)>,
    /// Derived pearl tickets, owner presence and the WAL ticket diff.
    pub(crate) chunk_tickets: crate::chunk_tickets::ChunkTickets,
    /// World revision when the previous entity phase read the world: a
    /// resting mob whose footprint chunks are no newer stays valid.
    pub entity_phase_revision: u64,
    /// Reused, non-authoritative entity-phase buffers; empty between ticks.
    entity_phase_scratch: EntityPhaseScratch,
    /// Reused per-tick explosion line-of-sight results; never persists or
    /// changes the ordered explosion application.
    pub(crate) explosion_exposure: crate::explosion::ExposureCache,
    /// Last swing tick per attacker (combat strength recharge).
    pub last_swing: HashMap<EntityId, u64>,
    /// Stand-specific player double-punch window; environmental hurt is unrelated.
    pub(crate) armor_stand_hits: HashMap<EntityId, u64>,
    /// World difficulty (commands).
    pub difficulty: crate::survival::Difficulty,
    pub(crate) journaled_difficulty: Option<crate::survival::Difficulty>,
    /// Active weather + remaining ticks (commands).
    pub weather: (crate::commands::Weather, i64),
    /// Retained client fade levels, independent of the target and cycle clock.
    pub(crate) rain_level: f32,
    pub(crate) thunder_level: f32,
    pub(crate) journaled_climate: Option<crate::weather::ClimateImage>,
    pub border: crate::command_border::Border,
    pub(crate) journaled_border: Option<crate::command_border::Border>,
    /// Game rules (commands).
    pub gamerules: HashMap<String, String>,
    pub(crate) gamerules_dirty: bool,
    /// Transient category counts and chunk-local spawn overlap candidates.
    pub(crate) spawn_census: crate::natural_spawn::SpawnCensus,
    /// Column rest-height cache: (block x, block z) -> rest feet y.
    /// Next per-actor intent sequence.
    seq_counter: HashMap<EntityId, u32>,
    /// Live block-entity storage by block position (chest/furnace contents).
    /// Journaled + checkpointed as post-images.
    pub block_entities: crate::redstone::custody::BlockEntities,
    /// Block-entity positions removed this tick (container broken): emitted
    /// as `JournalEntry::RemoveBlockEntity` and cleared at commit, so the
    /// tombstone shares the tick with the block change.
    pub block_entity_removed: Vec<(i32, i32, i32)>,
    /// Open container position per player entity (set by UseItemOn).
    pub open_containers: crate::redstone::custody::OpenContainers,
    pub(crate) vehicle_menus: crate::vehicles::VehicleMenus,
    /// Client-predicted block uses and respawns wait for native world admission.
    pub(crate) deferred_uses: Vec<PlayerInput>,
    pub(crate) sign_editors: HashMap<ConnectionId, crate::signs::Editor>,
    pub opaque_changes: Vec<(BlockPos, Option<Vec<u8>>)>,
    pub(crate) gateway_ages: HashMap<BlockPos, crate::end_portals::GatewayAge>,
    pub(crate) campfires: crate::campfire::Campfires,
    pub(crate) hives: crate::bees::Hives,
    pub(crate) jukeboxes: crate::jukebox::Jukeboxes,
    pub(crate) brewing: crate::brewing::BrewingStands,
    pub(crate) crafters: crate::crafter::Crafters,
    pub(crate) merge_shulker_placements: Vec<crate::shulker::Placement>,
    pub(crate) vibrations: crate::vibration::Bus,
    pub(crate) sculk: crate::sculk::SculkState,
    pub(crate) trial_spawners: crate::trial_spawner::Spawners,
    pub(crate) vaults: crate::vault::Vaults,
    pub(crate) shriekers: crate::shrieker::Shriekers,
    /// Players lying in a bed ([`crate::sleep`]).
    pub sleepers: HashMap<EntityId, crate::sleep::Sleeper>,
    /// Last journaled player / block-entity post-images. Images are
    /// absolute, so the journal writes one only when it differs from the
    /// last; an unchanged record is still covered by an earlier batch or
    /// the checkpoint.
    pub journaled_players: HashMap<EntityId, crate::journal::ReplayPlayer>,
    pub journaled_block_entities: HashMap<(i32, i32, i32), BlockEntityImage>,
    /// Attributed block/container history behind `/co` (dimension-local).
    pub(crate) block_log: crate::block_log::BlockLog,
    pub(crate) journaled_villagers: HashMap<EntityId, (u64, Box<crate::villager::VillagerState>)>,
    /// Last comparison tick and chunk anchor lifecycle clocks and crossings.
    pub(crate) journaled_entity_state:
        HashMap<EntityId, (u64, ChunkPos, Arc<crate::journal::EntityStateImage>)>,
    /// Touched-set journal caches (motion membership, clock rechecks, sweeps).
    pub(crate) journal_scan: crate::journal_touched::JournalScan,
    /// Creeper fuses, ranged mob charging and committed blasts.
    pub hostile: crate::hostile::HostileState,
    pub projectiles: crate::projectiles::ProjectileRuntime,
    pub end_fight: crate::end_fight::Fight,
    pub(crate) journaled_end_fight: Option<crate::end_fight::Fight>,
    pub raids: crate::raids::Raids,
    pub(crate) journaled_raids: Option<crate::raids::Raids>,
    pub(crate) trader_spawner: crate::mob_lanes::TraderSpawner,
    /// Vanilla restart-local 1200-tick admission timer; not saved data.
    pub(crate) trader_tick_delay: u32,
    pub(crate) journaled_trader_spawner: Option<crate::mob_lanes::TraderSpawner>,
    pub scoreboard: crate::scoreboard::Scoreboard,
    pub scoreboard_dirty: bool,
    pub bossbars_dirty: bool,
    pub(crate) dimension_globals: crate::dimensions::globals::Globals,
    pub(crate) villager_pois: crate::settlements::allocator::PoiCache,
    pub(crate) generated_entity_claims: std::collections::BTreeSet<String>,
    pub(crate) new_generated_entity_claims: Vec<String>,
    pub(crate) generated_entity_initial_scan: bool,
    pub(crate) settlements: crate::settlements::Registry,
    pub(crate) settlement_sequence: u64,
    pub(crate) autonomy_queue: crate::settlements::autonomy::AutonomyQueue,
    /// Per-connection certify-dialog corner marks (transient UI draft, never persisted).
    pub(crate) settlement_marks: HashMap<ConnectionId, crate::settlements::ui::Marks>,
    pub(crate) tick_budget: crate::tick_budget::Budget,
    /// Global `/tick` rate manager (moved between worlds with the other globals).
    pub(crate) tick_control: crate::tick_control::TickControl,
    /// Global `/bossbar` registry (moved between worlds with the other globals).
    pub bossbars: crate::bossbar::BossBars,
    /// `/locate` async searches owned by this dimension's tick (worker + drain).
    pub(crate) locate: crate::command_locate::LocateState,
}

/// Journaled block-entity contents: slots + furnace progress.
pub type BlockEntityImage = (
    Vec<mc_data::ItemStack>,
    Option<crate::journal::FurnaceImage>,
);

/// Live block-entity contents (slots + furnace progress).
#[derive(Debug, Clone, Default)]
pub struct BlockEntityState {
    pub slots: Vec<mc_data::ItemStack>,
    pub furnace: Option<crate::inventory::furnace::FurnaceState>,
}

impl BlockEntityState {
    /// Journal/checkpoint image of the furnace progress.
    pub(crate) fn furnace_image(&self) -> Option<crate::journal::FurnaceImage> {
        self.furnace
            .as_ref()
            .map(|progress| crate::journal::FurnaceImage {
                lit_remaining: progress.lit_remaining,
                lit_total: progress.lit_total,
                cook_progress: progress.cook_progress,
                cook_total: progress.cook_total,
            })
    }
}

/// One child spawn staged by merge.
#[derive(Debug, Clone)]
pub struct MergeSpawn {
    pub pos: [f64; 3],
    pub type_id: u32,
    pub health: f32,
    pub wool: Option<u8>,
    pub taming: Option<crate::taming::TamingState>,
    pub advancement_breeding: Option<(EntityId, serde_json::Value)>,
}

impl TickState {
    pub fn new(
        registry: Arc<CoreRegistry>,
        world: ChunkWorld,
        sim_distance: i32,
        outbox_cap: usize,
    ) -> Self {
        Self {
            config_sim_distance: sim_distance,
            config_view_distance: sim_distance,
            active_player_centers: Vec::new(),
            active_ticket_ranges: Vec::new(),
            active_sim_radius: -1,
            registry,
            world,
            dimension: crate::dimensions::Dimension::Overworld,
            other_dimensions: Default::default(),
            dimension_requests: Vec::new(),
            dimension_respawns: Vec::new(),
            dimension_routes: Vec::new(),
            entities: EntityStore::default(),
            connections: Connections::default(),
            require_plugin_login: false,
            locked_sessions: BTreeSet::new(),
            max_airtime_ticks: 80,
            mob_griefing: crate::config::MobGriefingRules::default(),
            players: HashMap::default(),
            offline_players: HashMap::default(),
            advancement_events: Vec::new(),
            advancement_reward_spawns: Vec::new(),
            native_globals: Default::default(),
            plugins: Default::default(),
            plugin_runtime: None,
            build_queries: None,
            plugin_spawned: Vec::new(),
            plugin_block_writes: Vec::new(),
            plugin_players: Default::default(),
            plugin_damage: Vec::new(),
            plugin_interactions: HashMap::default(),
            outbox: HashMap::default(),
            outbox_cap,
            disconnects: Vec::new(),
            tick: 0,
            wall_clock: crate::advancements::system_unix_seconds,
            entity_salt: [0; crate::entity_uuid::SALT_LEN],
            day_time: 0,
            merge_block_writes: Vec::new(),
            block_phase_writes: Vec::new(),
            serial_entity_slots: Vec::new(),
            serial_entity_marks: Vec::new(),
            contact_hazards_scratch: Default::default(),
            portal_match_cache: Default::default(),
            portal_contact_stamps: Vec::new(),
            merge_broken: Vec::new(),
            merge_command_broken: Vec::new(),
            merge_container_contents: Vec::new(),
            merge_closed_viewers: Vec::new(),
            merge_damage: Vec::new(),
            merge_consumed_items: HashSet::default(),
            merge_spawns: Vec::new(),
            merge_life_drops: Vec::new(),
            merge_gives: Vec::new(),
            fluid_ticks: crate::fluids::FluidTicks::default(),
            block_ticks: crate::block_ticks::BlockTicks::default(),
            redstone: crate::redstone::RedstoneState::default(),
            redstone_containers: crate::redstone::containers::ContainerState::default(),
            farm_sections: crate::farming::FarmSections::default(),
            merge_pickups: Vec::new(),
            appearance_changed: Vec::new(),
            left_players: Vec::new(),
            leash_knots: HashMap::default(),
            tickets: BTreeMap::new(),
            chunk_tickets: crate::chunk_tickets::ChunkTickets::default(),
            entity_phase_revision: 0,
            entity_phase_scratch: EntityPhaseScratch::default(),
            explosion_exposure: crate::explosion::ExposureCache::default(),
            last_swing: HashMap::default(),
            armor_stand_hits: HashMap::default(),
            // Vanilla dedicated-server default; the join sends it.
            difficulty: crate::survival::Difficulty::Easy,
            journaled_difficulty: None,
            weather: (crate::commands::Weather::Clear, 0),
            rain_level: 0.0,
            thunder_level: 0.0,
            journaled_climate: None,
            border: Default::default(),
            journaled_border: None,
            gamerules: HashMap::default(),
            gamerules_dirty: false,
            spawn_census: Default::default(),
            seq_counter: HashMap::default(),
            block_entities: Default::default(),
            block_entity_removed: Vec::new(),
            open_containers: Default::default(),
            vehicle_menus: crate::vehicles::VehicleMenus::default(),
            deferred_uses: Vec::new(),
            sign_editors: HashMap::default(),
            opaque_changes: Vec::new(),
            gateway_ages: HashMap::default(),
            campfires: crate::campfire::Campfires::default(),
            hives: crate::bees::Hives::default(),
            jukeboxes: crate::jukebox::Jukeboxes::default(),
            brewing: crate::brewing::BrewingStands::default(),
            crafters: crate::crafter::Crafters::default(),
            merge_shulker_placements: Vec::new(),
            vibrations: crate::vibration::Bus::default(),
            sculk: crate::sculk::SculkState::default(),
            trial_spawners: Default::default(),
            vaults: Default::default(),
            shriekers: Default::default(),
            sleepers: HashMap::default(),
            journaled_players: HashMap::default(),
            journaled_block_entities: HashMap::default(),
            block_log: Default::default(),
            journaled_villagers: HashMap::default(),
            journaled_entity_state: HashMap::default(),
            journal_scan: Default::default(),
            hostile: crate::hostile::HostileState::default(),
            projectiles: crate::projectiles::ProjectileRuntime::default(),
            end_fight: Default::default(),
            journaled_end_fight: None,
            raids: Default::default(),
            journaled_raids: None,
            trader_spawner: Default::default(),
            trader_tick_delay: 1200,
            journaled_trader_spawner: None,
            scoreboard: crate::scoreboard::Scoreboard::default(),
            scoreboard_dirty: false,
            bossbars_dirty: false,
            bossbars: Default::default(),
            dimension_globals: Default::default(),
            villager_pois: Default::default(),
            generated_entity_claims: Default::default(),
            new_generated_entity_claims: Vec::new(),
            generated_entity_initial_scan: false,
            settlements: Default::default(),
            settlement_sequence: 0,
            autonomy_queue: Default::default(),
            settlement_marks: HashMap::default(),
            tick_budget: Default::default(),
            tick_control: Default::default(),
            locate: Default::default(),
        }
    }

    pub(crate) fn session_locked(&self, connection: ConnectionId) -> bool {
        self.require_plugin_login && self.locked_sessions.contains(&connection)
    }

    pub(crate) fn entity_session_locked(&self, entity: EntityId) -> bool {
        self.require_plugin_login
            && self
                .players
                .get(&entity)
                .is_some_and(|player| self.locked_sessions.contains(&player.connection))
    }

    pub(crate) fn next_seq(&mut self, actor: EntityId) -> u32 {
        let seq = self.seq_counter.get(&actor).copied().unwrap_or(0);
        self.seq_counter.insert(actor, seq + 1);
        seq
    }

    /// Reliable per-player push. On cap overflow the queue is sealed with a
    /// terminal `Disconnect` (always last) and the connection is scheduled
    /// for Leave-equivalent removal on a later tick; further pushes are
    /// dropped so nothing can land after `Disconnect`.
    pub fn push_player(&mut self, connection: ConnectionId, change: PlayerChange) {
        if self
            .disconnects
            .iter()
            .any(|(sealed, _)| *sealed == connection)
        {
            return;
        }
        crate::vibration::menu(self, connection, &change);
        let queue = self.outbox.entry(connection).or_default();
        if queue.len() >= self.outbox_cap {
            queue.push(PlayerChange::Disconnect {
                reason: "reliable queue overflow".to_owned(),
            });
            self.disconnects.push((connection, self.tick));
            return;
        }
        queue.push(change);
    }

    /// Fence movement when the authoritative sync is created, before the
    /// off-tick edge can send it or admit an older queued client pose.
    pub(crate) fn correct_position(
        &mut self,
        connection: ConnectionId,
        position: [f64; 3],
        yaw: f32,
        pitch: f32,
    ) {
        let Some(entity) = self.connections.entity_of(connection) else {
            return;
        };
        let Some(player) = self.players.get_mut(&entity) else {
            return;
        };
        let teleport_id = player.begin_teleport();
        self.push_player(
            connection,
            PlayerChange::PositionCorrected {
                teleport_id,
                position,
                yaw,
                pitch,
            },
        );
    }

    pub(crate) fn stage_command_break(&mut self, pos: BlockPos, broken: u32) -> bool {
        let drops = if crate::command_rules::boolean(self, "block_drops") {
            crate::block_drop_context::roll(
                self,
                pos,
                broken,
                &mc_data::ItemStack::EMPTY,
                self.tick ^ ((pos.x as u64) << 40) ^ ((pos.z as u64) << 16) ^ pos.y as u64,
            )
        } else {
            Ok(Vec::new())
        };
        let Some(drops) = crate::block_drop_context::admit(self, pos, drops) else {
            return false;
        };
        self.stage_container_removal(pos, !crate::block_drop_context::retains_contents(&drops));
        self.merge_command_broken.push((pos, broken, drops));
        true
    }

    pub(crate) fn stage_command_container_removal(&mut self, pos: BlockPos) {
        self.stage_container_removal(pos, false);
    }

    pub(crate) fn stage_container_removal(&mut self, pos: BlockPos, drop_contents: bool) {
        crate::plugin_inventories::invalidate_block(self, pos);
        let cell = (pos.x, pos.y, pos.z);
        crate::sculk::removed(self, pos);
        if self
            .merge_container_contents
            .iter()
            .any(|(other, _)| *other == cell)
        {
            return;
        }
        let record = self.block_entities.get(&cell);
        let opaque = self
            .world
            .chunk(ChunkPos {
                x: pos.x.div_euclid(16),
                z: pos.z.div_euclid(16),
            })
            .is_some_and(|chunk| chunk.block_entities.contains_key(&pos));
        if record.is_none() && !opaque {
            return;
        }
        let slots = if drop_contents {
            record
                .map(|record| record.slots.clone())
                .unwrap_or_default()
        } else {
            Vec::new()
        };
        self.merge_container_contents.push((cell, slots));
        let viewers: Vec<EntityId> = self
            .open_containers
            .iter()
            .filter(|(_, open)| container_cells(self, **open).contains(&cell))
            .map(|(viewer, _)| *viewer)
            .collect();
        self.merge_closed_viewers
            .extend(viewers.into_iter().map(|viewer| (viewer, cell)));
    }

    /// Kind-specific preconditions against merge state: a second feed of
    /// the same animal, or a second breed with the same partner, in one tick
    /// is rejected whole (pays nothing).
    fn kind_valid(&self, intent: &Intent) -> bool {
        let kind = |id: EntityId| {
            self.entities
                .slot_of(id)
                .map(|slot| self.entities.body(slot).kind)
        };
        if !intent.footprint.block_write.is_empty()
            && kind(intent.actor).is_some_and(|kind| matches!(kind, EntityKind::Mob { .. }))
            && !crate::command_rules::boolean(self, "mob_griefing")
        {
            return false;
        }
        match &intent.kind {
            IntentKind::FeedAnimal { target } => {
                self.entities.slot_of(*target).is_some_and(|slot| {
                    crate::entity_life::can_feed(self.entities.body(slot), self.entities.row(slot))
                        && crate::entity_life::can_breed(
                            self,
                            self.entities.body(slot),
                            self.entities.row(slot),
                        )
                })
            }
            IntentKind::DolphinFeed { plan } => crate::dolphin::valid(self, intent.actor, plan),
            IntentKind::HarvestAnimal { plan } => {
                crate::entity_life::valid(self, intent.actor, plan)
            }
            IntentKind::MountInteract { plan } => crate::mounts::valid(self, intent.actor, plan),
            IntentKind::Breed { partner, .. } => [intent.actor, *partner].into_iter().all(|id| {
                self.entities.slot_of(id).is_some_and(|slot| {
                    crate::entity_life::breed_ready(
                        self.entities.body(slot),
                        self.entities.row(slot),
                    ) && crate::entity_life::can_breed(
                        self,
                        self.entities.body(slot),
                        self.entities.row(slot),
                    )
                })
            }),
            _ => true,
        }
    }

    /// Validate one footprint against current merge state.
    pub(crate) fn footprint_valid(&self, footprint: &Footprint) -> bool {
        for id in &footprint.entity_reads {
            if self
                .entities
                .slot_of(crate::end_fight::attack_target(self, *id).0)
                .is_none()
            {
                return false;
            }
        }
        for id in &footprint.entity_writes {
            if self
                .entities
                .slot_of(crate::end_fight::attack_target(self, *id).0)
                .is_none()
            {
                return false;
            }
        }
        if let Some(item) = footprint.item_take {
            let Some(slot) = self.entities.slot_of(item) else {
                return false;
            };
            // One winner per item per tick: a staged pickup for the same
            // slot rejects later claimants (loser rejects whole).
            if self
                .merge_pickups
                .iter()
                .any(|(staged, _, _)| *staged == slot)
            {
                return false;
            }
        }
        for (pos, expect) in &footprint.block_expect {
            if self.world.block_at(pos.x, pos.y, pos.z) != Some(*expect) {
                return false;
            }
            // A previously accepted intent already rewrote this position.
            if self
                .merge_block_writes
                .iter()
                .any(|(other, _)| other == pos)
            {
                return false;
            }
        }
        for claim in &footprint.inv {
            let Some(player) = self.players.get(&claim.owner) else {
                return false;
            };
            let slot = player.inventory.slots.get(claim.slot as usize);
            if slot.is_none_or(mc_data::ItemStack::is_empty) {
                if claim.take > 0 || claim.expect_count != 0 {
                    return false;
                }
            } else if let Some(slot) = slot {
                if slot.item_id != claim.expect_item || slot.count != claim.expect_count {
                    return false;
                }
                if slot.count < claim.take {
                    return false;
                }
            }
        }
        !crate::plugin_inventories::below_intent_floors(self, &footprint.inv)
    }

    /// Apply one validated intent's effect to merge staging.
    fn apply_intent(&mut self, intent: &Intent) -> bool {
        match &intent.kind {
            IntentKind::AuthorizedBuild { .. } => return false,
            IntentKind::PhysicalAttack { started_tick } => {
                if let Some((slot, impact, source)) =
                    crate::physical_attacks::commit_attack(self, intent.actor, *started_tick)
                {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: impact.damage_hp,
                            kind: crate::survival::DamageKind::Melee,
                            source: Some(source),
                            physical: Some(impact),
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::PhysicalDamageEntity {
                target,
                impact,
                source,
            } => {
                if let Some(slot) = self.entities.slot_of(*target) {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: impact.damage_hp,
                            kind: crate::survival::DamageKind::Projectile,
                            source: *source,
                            physical: Some(*impact),
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::DamageEntity {
                target,
                amount,
                kind,
                source,
            } => {
                let (resolved, non_head) = crate::end_fight::attack_target(self, *target);
                if let Some(slot) = self.entities.slot_of(resolved) {
                    self.merge_damage.push((
                        slot,
                        DamagePiece {
                            amount: if non_head {
                                *amount / 4.0 + (*amount).min(1.0)
                            } else {
                                *amount
                            },
                            kind: *kind,
                            source: *source,
                            physical: None,
                            custom_armor: false,
                        },
                    ));
                }
            }
            IntentKind::PickupItem { item, count } => {
                // Reservation-free: the pickup reserves nothing at intent
                // time (capacity is known only at commit). Staging the
                // intent is enough; commit removes exactly the slots whose
                // remainder is empty.
                let (Some(item_slot), Some(actor_slot)) = (
                    self.entities.slot_of(*item),
                    self.entities.slot_of(intent.actor),
                ) else {
                    return false;
                };
                self.merge_pickups.push((item_slot, actor_slot, *count));
            }
            IntentKind::BreakBlock { pos } => {
                // Drops read the broken state now (merge precedes writes).
                if let Some(broken) = self.world.block_at(pos.x, pos.y, pos.z) {
                    let wear = self.block_break_tool_wear(intent.actor);
                    let Ok(wear) = self.player_tool_wear_plan(intent.actor, wear) else {
                        return false;
                    };
                    let xp = self.block_break_experience(intent.actor, broken);
                    let rolled =
                        crate::block_drop_context::player_drops(self, intent.actor, *pos, broken);
                    let Some(drops) = crate::block_drop_context::admit(self, *pos, rolled) else {
                        return false;
                    };
                    if let Ok(Some(bees)) = crate::bees::inhabited_bees(self, *pos) {
                        let block = self
                            .registry
                            .blocks
                            .by_id(mc_world::BlockStateId(broken))
                            .map(|block| block.block.id.as_str().to_owned());
                        let tool = self
                            .players
                            .get(&intent.actor)
                            .and_then(|player| {
                                player
                                    .inventory
                                    .slots
                                    .get(usize::from(player.inventory.selected.min(8)))
                            })
                            .map(|tool| crate::advancements::item_snapshot(self, tool));
                        crate::advancements::trigger(
                            self,
                            intent.actor,
                            "minecraft:bee_nest_destroyed",
                            serde_json::json!({"block":block,"item":tool,"num_bees_inside":bees}),
                        );
                    }
                    self.record_block_mined(intent.actor, broken);
                    self.apply_player_tool_wear(intent.actor, wear);
                    self.stage_container_removal(
                        *pos,
                        !crate::block_drop_context::retains_contents(&drops),
                    );
                    self.merge_broken
                        .push((intent.actor, *pos, broken, drops, xp));
                }
                mining::cancel(self, intent.actor);
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
            }
            IntentKind::UseBlock { pos, hand } => {
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(hand) = hand
                    && let Some((_, after)) = intent
                        .footprint
                        .block_write
                        .iter()
                        .find(|(cell, _)| cell == pos)
                    && self
                        .registry
                        .blocks
                        .by_id(mc_world::BlockStateId(*after))
                        .is_some_and(|block| block.block.id.path() == "respawn_anchor")
                {
                    let item = self.players.get(&intent.actor).map(|player| {
                        player.inventory.slots[held_slot_index(player, *hand)].clone()
                    });
                    crate::advancements::accepted_block(
                        self,
                        intent.actor,
                        "minecraft:item_used_on_block",
                        *pos,
                        Some(*after),
                        item,
                    );
                }
            }
            IntentKind::Bucket { pos, give } => {
                // Emptying into a washable block (grass, flowers) drops it.
                if let Some(old) = self.world.block_at(pos.x, pos.y, pos.z)
                    && old != self.registry.states.air
                    && crate::fluids::fluid_at(&self.registry.states, old).is_none()
                    && intent.footprint.block_write.iter().any(|(cell, new)| {
                        *cell == *pos
                            && crate::fluids::fluid_at(&self.registry.states, *new).is_some()
                    })
                {
                    let wear = self.block_break_tool_wear(intent.actor);
                    let Ok(wear) = self.player_tool_wear_plan(intent.actor, wear) else {
                        return false;
                    };
                    let xp = self.block_break_experience(intent.actor, old);
                    let rolled =
                        crate::block_drop_context::player_drops(self, intent.actor, *pos, old);
                    let Some(drops) = crate::block_drop_context::admit(self, *pos, rolled) else {
                        return false;
                    };
                    self.record_block_mined(intent.actor, old);
                    self.apply_player_tool_wear(intent.actor, wear);
                    self.stage_container_removal(
                        *pos,
                        !crate::block_drop_context::retains_contents(&drops),
                    );
                    self.merge_broken.push((intent.actor, *pos, old, drops, xp));
                }
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(item) = give {
                    self.merge_gives.push((intent.actor, *item));
                }
                if let Some(item) =
                    give.or_else(|| intent.footprint.inv.iter().find_map(|claim| claim.put_item))
                    && self
                        .registry
                        .items
                        .name_of(item)
                        .is_some_and(|id| id.path() != "bucket")
                {
                    let evidence =
                        crate::advancements::item_snapshot(self, &mc_data::ItemStack::new(item, 1));
                    crate::advancements::trigger(
                        self,
                        intent.actor,
                        "minecraft:filled_bucket",
                        serde_json::json!({"item":evidence}),
                    );
                }
            }
            IntentKind::Cultivate { wear } => {
                let Ok(plan) = self.player_tool_wear_plan(intent.actor, *wear) else {
                    return false;
                };
                self.apply_player_tool_wear(intent.actor, plan);
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
            }
            IntentKind::BlockTool { wear, give } => {
                let Ok(plan) = self.player_tool_wear_plan(intent.actor, *wear) else {
                    return false;
                };
                self.apply_player_tool_wear(intent.actor, plan);
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if let Some(item) = give {
                    self.merge_gives.push((intent.actor, *item));
                }
            }
            IntentKind::ComposterFill { pos, maturing } => {
                self.merge_block_writes
                    .extend_from_slice(&intent.footprint.block_write);
                crate::composter::schedule_fill(self, *pos, *maturing);
            }
            IntentKind::PlaceBlock { pos, state } => {
                let staged = self.merge_block_writes.len();
                for (write_pos, write_state) in &intent.footprint.block_write {
                    self.merge_block_writes.push((*write_pos, *write_state));
                }
                if crate::bees::place_from_item(self, intent).is_err()
                    || crate::shulker::place_from_item(self, intent).is_err()
                {
                    self.merge_block_writes.truncate(staged);
                    return false;
                }
                crate::block_entity_defaults::accepted_placement(self, *pos, *state);
                let held = intent
                    .footprint
                    .inv
                    .iter()
                    .find_map(|claim| {
                        self.players
                            .get(&claim.owner)
                            .and_then(|player| player.inventory.slots.get(claim.slot as usize))
                    })
                    .cloned();
                crate::advancements::accepted_block(
                    self,
                    intent.actor,
                    "minecraft:placed_block",
                    *pos,
                    Some(*state),
                    held.clone(),
                );
                crate::advancements::accepted_block(
                    self,
                    intent.actor,
                    "minecraft:item_used_on_block",
                    *pos,
                    Some(*state),
                    held,
                );
                crate::plugin::block_event(self, intent.actor, *pos, *state, true);
                if self
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(*state))
                    .is_some_and(crate::signs::editable)
                    && let Some(connection) = self.connections.entity_connection(intent.actor)
                {
                    self.sign_editors.insert(
                        connection,
                        crate::signs::Editor {
                            position: *pos,
                            block: *state,
                            opened_tick: self.tick,
                            front: true,
                        },
                    );
                    self.push_player(
                        connection,
                        PlayerChange::SignEditorOpened {
                            position: *pos,
                            front: true,
                            block: *state,
                        },
                    );
                }
            }
            IntentKind::SpawnChild {
                pos,
                type_id,
                health,
            } => {
                self.merge_spawns.push(MergeSpawn {
                    pos: *pos,
                    type_id: *type_id,
                    health: *health,
                    wool: None,
                    taming: None,
                    advancement_breeding: None,
                });
            }
            IntentKind::Breed {
                partner,
                child_pos,
                child_type,
            } => {
                crate::entity_life::breed(self, intent.actor, *partner, *child_pos, *child_type);
            }
            IntentKind::FeedAnimal { target } => {
                crate::entity_life::feed(self, *target);
                if let Some(actor) = self.entities.slot_of(intent.actor)
                    && let Some(target) = self.entities.slot_of(*target)
                {
                    self.entities.row_mut(target).entity_life.taming.love_cause =
                        Some(self.entities.row(actor).uuid);
                }
            }
            IntentKind::DolphinFeed { plan } => {
                crate::dolphin::feed(self, plan);
            }
            IntentKind::HarvestAnimal { plan } => {
                crate::entity_life::apply(self, intent.actor, plan);
                return true;
            }
            IntentKind::MountInteract { plan } => {
                crate::mounts::apply(self, intent.actor, plan);
                return true;
            }
            IntentKind::Explode {
                center,
                power,
                mob_grief,
                kill_credit,
            } => {
                crate::explosion::apply(
                    self,
                    Some(intent.actor),
                    *center,
                    *power,
                    *mob_grief,
                    *kill_credit,
                );
            }
            IntentKind::EnvironmentalExplosion { center, power } => {
                for (pos, block) in &intent.footprint.block_write {
                    self.world.set_block(pos.x, pos.y, pos.z, *block);
                    self.merge_block_writes.push((*pos, *block));
                }
                crate::explosion::apply(self, None, *center, *power, None, None);
            }
            IntentKind::SetRespawn { pos } => {
                let rotation = self
                    .entities
                    .slot_of(intent.actor)
                    .map(|slot| self.entities.rot(slot));
                if let Some(player) = self.players.get_mut(&intent.actor) {
                    player.respawn_point = Some([pos.x, pos.y, pos.z]);
                    player.respawn_dimension = self.dimension;
                    player.respawn_forced = false;
                    player.respawn_yaw = rotation.map_or(0.0, |rotation| rotation[0]);
                    player.respawn_pitch = 0.0;
                }
            }
        }
        crate::vibration::intent(self, intent);
        // Inventory claims apply with the intent, whole.
        for claim in &intent.footprint.inv {
            if let Some(player) = self.players.get_mut(&claim.owner)
                && let Some(slot) = player.inventory.slots.get_mut(claim.slot as usize)
            {
                if matches!(
                    intent.kind,
                    IntentKind::PlaceBlock { .. }
                        | IntentKind::Bucket { .. }
                        | IntentKind::Cultivate { .. }
                        | IntentKind::BlockTool { .. }
                        | IntentKind::UseBlock { .. }
                ) && claim.owner == intent.actor
                {
                    player
                        .statistics
                        .increment(StatType::Used, claim.expect_item, 1);
                }
                if slot.is_empty() {
                    if let Some(put) = claim.put_item {
                        *slot = mc_data::ItemStack::new(put, claim.put_count);
                    }
                } else {
                    let rest = slot.count - claim.take;
                    if rest <= 0 && claim.put_item.is_none() {
                        *slot = mc_data::ItemStack::EMPTY;
                    } else if let Some(put) = claim.put_item {
                        *slot = mc_data::ItemStack::new(put, claim.put_count);
                    } else if rest <= 0 {
                        *slot = mc_data::ItemStack::EMPTY;
                    } else {
                        slot.count = rest;
                    }
                }
            }
        }
        // Winner pays visibly: sync the inventory after the debit above.
        if matches!(
            intent.kind,
            IntentKind::PlaceBlock { .. }
                | IntentKind::FeedAnimal { .. }
                | IntentKind::Bucket { .. }
                | IntentKind::Cultivate { .. }
                | IntentKind::ComposterFill { .. }
        ) {
            let sync = self
                .players
                .get(&intent.actor)
                .map(|player| player.inventory_contents());
            if let (Some(contents), Some(connection)) =
                (sync, self.connections.by_entity.get(&intent.actor).copied())
            {
                self.push_player(connection, contents);
            }
        }
        true
    }

    /// Merge: sort intents, validate each footprint whole, apply or reject.
    /// Returns accepted count.
    pub fn merge(&mut self, intents: &mut Vec<Intent>) -> usize {
        sort_intents(intents);
        let staged = std::mem::take(intents);
        let mut accepted = 0;
        let mut interactions = std::mem::take(&mut self.plugin_interactions);
        for intent in &staged {
            let writes = self.merge_block_writes.len();
            if let IntentKind::AuthorizedBuild { authorization } = &intent.kind {
                if crate::build_precommit::commit_authorized(
                    self,
                    authorization,
                    intent.seq,
                    |state, native| {
                        state.footprint_valid(&native.footprint) && state.kind_valid(native)
                    },
                    Self::apply_intent,
                ) {
                    accepted += 1;
                    crate::block_log::note_merge(self, intent.actor, writes);
                    crate::plugin::enqueue_result(
                        self,
                        interactions.remove(&(intent.actor, intent.seq)),
                    );
                }
                continue;
            }
            if self.footprint_valid(&intent.footprint)
                && self.kind_valid(intent)
                && self.apply_intent(intent)
            {
                accepted += 1;
                crate::block_log::note_merge(self, intent.actor, writes);
                crate::plugin::enqueue_result(
                    self,
                    interactions.remove(&(intent.actor, intent.seq)),
                );
            } else if matches!(
                intent.kind,
                IntentKind::PlaceBlock { .. }
                    | IntentKind::FeedAnimal { .. }
                    | IntentKind::Cultivate { .. }
                    | IntentKind::ComposterFill { .. }
            ) {
                // Rejected placement/feed paid nothing; resync the held slot
                // so client-side prediction snaps back.
                let sync = self
                    .players
                    .get(&intent.actor)
                    .map(|player| player.inventory_contents());
                if let (Some(contents), Some(connection)) =
                    (sync, self.connections.by_entity.get(&intent.actor).copied())
                {
                    self.push_player(connection, contents);
                }
            }
        }
        accepted
    }

    /// Commit staged merge effects: damage, pickups, spawns, animal harvesting,
    /// block drops. Returns `(deaths, events, spawned_item_entities)`.
    /// Death drops (player inventory + death containers) ride the returned
    /// spawn list so `run_tick` journals them in the same tick; committed
    /// full-pickup removals ride the fourth element so `run_tick` journals
    /// their tombstones in the same tick.
    pub fn commit_merge_effects(
        &mut self,
    ) -> (
        Vec<EntityId>,
        Vec<EntityEvent>,
        Vec<EntityId>,
        Vec<EntityId>,
    ) {
        let (deaths, mut events, mut spawned) = self.commit_damage();
        crate::entity_life::commit_drops(self, &mut spawned);
        // Pickups: move item counts into the collector's inventory. Only a
        // fully-consumed ground stack is queued for removal; a partial or
        // zero-capacity pickup keeps its reservation-free remainder on the
        // ground (the `merge_consumed_items` reservation is commit-scoped:
        // commit drains it into removals only for empty remainders).
        let pickups = std::mem::take(&mut self.merge_pickups);
        let mut committed_removals: Vec<EntityId> = Vec::new();
        for (item_slot, collector_slot, _count) in pickups {
            if (item_slot.0 as usize) >= self.entities.len()
                || (collector_slot.0 as usize) >= self.entities.len()
            {
                continue;
            }
            let item_id = self.entities.body(item_slot).id;
            let collector_id = self.entities.body(collector_slot).id;
            if !self.entities.body(collector_slot).alive || self.entity_session_locked(collector_id)
            {
                continue;
            }
            let item_row = self.entities.row(item_slot).clone();
            if let Some(stack) = item_row.item
                && let Some(player) = self.players.get_mut(&collector_id)
            {
                let rules = self.registry.item_rules();
                let mut moving = stack.full_stack();
                let before = moving.count;
                player.inventory.add(&rules, &mut moving);
                let taken = before - moving.count;
                if taken > 0 {
                    player
                        .statistics
                        .increment(StatType::PickedUp, stack.item_id, taken);
                }
                if moving.is_empty() {
                    committed_removals.push(item_id);
                } else {
                    // Partial pickup: the remainder stays on the ground.
                    let row = self.entities.row_mut(item_slot);
                    if let Some(slot) = row.item.as_mut() {
                        slot.count = moving.count;
                        slot.stack = Some(moving);
                    }
                    let appearance = std::sync::Arc::make_mut(&mut row.appearance);
                    appearance.item = row.item.as_ref().map(|slot| slot.full_stack());
                    self.appearance_changed.push(item_id);
                }
                events.push(EntityEvent::Pickup {
                    item: item_id,
                    collector: collector_id,
                    count: taken.clamp(0, 255) as u8,
                });
                crate::plugin::pickup_event(self, collector_id, stack.item_id, taken);
                if taken > 0
                    && !self.registry.tags.contains_raw_id(
                        "minecraft:item",
                        "minecraft:dampens_vibrations",
                        stack.item_id as i32,
                    )
                {
                    let pos = self.entities.pos(collector_slot);
                    crate::vibration::emit(
                        self,
                        pos,
                        crate::vibration::GameEvent::EntityInteract,
                        Some(collector_id),
                    );
                }
                // Slot sync for the collector's inventory.
                if let Some(connection) = self.connections.by_entity.get(&collector_id).copied()
                    && let Some(player) = self.players.get(&collector_id)
                {
                    let burst = player.inventory_contents();
                    self.push_player(connection, burst);
                }
            }
            // No inventory (mob collector or despawned slot): the entity is
            // NOT removed — nothing took custody. It stays on the ground.
        }
        // Containers take custody before this commit. Their fully consumed
        // rows need the same removal/tombstone path as a player pickup;
        // reservations alone never delete a nonempty remainder.
        for slot in &self.merge_consumed_items {
            if (slot.0 as usize) < self.entities.len() {
                let body = self.entities.body(*slot);
                let row = self.entities.row(*slot);
                if matches!(body.kind, EntityKind::Item { .. })
                    && row.item.as_ref().is_some_and(|item| item.count <= 0)
                {
                    committed_removals.push(body.id);
                }
            }
        }
        self.merge_consumed_items.clear();
        // Remove committed full pickups (by id: slots may have shifted under
        // partial-pickup writes) and the dead (by id, dead-inclusive: the
        // death flag above set alive=false before this lookup). Highest
        // slots first so swap-remove never invalidates a pending slot.
        let mut dead_slots: Vec<EntitySlot> = committed_removals
            .iter()
            .filter_map(|id| self.entities.slot_of_any(*id))
            .collect();
        crate::projectiles::queue_owned_pearls_on_deaths(self, &deaths, &mut dead_slots);
        crate::vehicles::queue_expired_removals(self, &mut dead_slots);
        // Dead mobs/items leave the store (tombstoned via `removed`); dead
        // PLAYERS stay addressable (alive=false row kept) so Respawn can
        // revive and persistence journals the emptied post-image. Their
        // drops still spawned above; only the row removal is skipped.
        for id in &deaths {
            let Some(slot) = self.entities.slot_of_any(*id) else {
                continue;
            };
            let is_player = matches!(
                self.entities.body(slot).kind,
                crate::entities::EntityKind::Player { .. }
            );
            if is_player {
                continue;
            }
            dead_slots.push(slot);
        }
        dead_slots.sort_by_key(|slot| std::cmp::Reverse(slot.0));
        dead_slots.dedup();
        let mut removed: Vec<EntityId> = Vec::with_capacity(dead_slots.len());
        for slot in dead_slots {
            if (slot.0 as usize) < self.entities.len() {
                removed.push(self.entities.body(slot).id);
                self.remove_entity(slot);
            }
        }
        // Spawns.
        let spawns = std::mem::take(&mut self.merge_spawns);
        for spawn in spawns {
            let id = self.spawn_mob(spawn.type_id, spawn.pos, spawn.health);
            crate::entity_life::make_child(self, id);
            if let Some(taming) = spawn.taming
                && let Some(slot) = self.entities.slot_of(id)
            {
                let (body, row) = self.entities.parts_mut(slot);
                row.entity_life.tamed = taming.owner.is_some();
                row.entity_life.taming = taming;
                crate::entity_life::restore_appearance(body, row);
            }
            if let Some(wool) = spawn.wool
                && let Some(slot) = self.entities.slot_of(id)
            {
                set_wool(
                    Arc::make_mut(&mut self.entities.row_mut(slot).appearance),
                    wool,
                );
            }
            if let Some((player, mut evidence)) = spawn.advancement_breeding {
                evidence["child"] = crate::advancements::entity_snapshot(self, id);
                crate::advancements::trigger(self, player, "minecraft:bred_animals", evidence);
            }
            spawned.push(id);
            let xp = 1 + ((self.tick ^ id.0 as u64) % 7) as i32;
            crate::experience_orbs::award(self, spawn.pos, xp, &mut spawned);
        }
        // Block drops: survival breaks spawn loot at the block centre;
        // Accepted breaks retain pre-wear loot and experience; wear was paid at admission.
        for (actor, pos, broken, drops, xp) in std::mem::take(&mut self.merge_broken) {
            self.commit_break_drops(actor, pos, broken, drops, xp, &mut spawned);
            crate::plugin::block_event(self, actor, pos, broken, false);
        }
        for (pos, _broken, drops) in std::mem::take(&mut self.merge_command_broken) {
            self.spawn_block_loot(pos, drops, pos.x as u64 ^ pos.z as u64, &mut spawned);
        }
        self.commit_gives(&mut spawned);
        // Container breaks: spawn each broken container's contents as drops
        // at the block centre (journaled via `spawned`), remove its storage,
        // mark the chunk dirty, stage the `RemoveBlockEntity` tombstone.
        for ((x, y, z), slots) in std::mem::take(&mut self.merge_container_contents) {
            let centre = [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5];
            for stack in slots {
                if stack.is_empty() {
                    continue;
                }
                let drop = spawn_item_entity(self, centre, stack);
                spawned.push(drop);
            }
            self.block_entities.remove(&(x, y, z));
            self.brewing.remove(&(x, y, z));
            self.crafters.remove(&(x, y, z));
            let pos = BlockPos { x, y, z };
            if self
                .world
                .chunk(ChunkPos {
                    x: x.div_euclid(16),
                    z: z.div_euclid(16),
                })
                .is_some_and(|chunk| chunk.block_entities.contains_key(&pos))
            {
                self.world.set_opaque_block_entity(pos, None);
                self.opaque_changes.push((pos, None));
            }
            self.world.mark_block_entity_dirty(BlockPos { x, y, z });
            self.block_entity_removed.push((x, y, z));
        }
        // Close every viewer of a broken container with a MenuClosed.
        for (viewer, (x, y, z)) in std::mem::take(&mut self.merge_closed_viewers) {
            if self.open_containers.remove(&viewer).is_none() {
                continue;
            }
            let centre = [x as f64 + 0.5, y as f64 + 0.5, z as f64 + 0.5];
            let connection = self.connections.entity_connection(viewer);
            // Capture the open window id BEFORE `close()` takes the menu
            // (the client needs the real window, not a constant).
            let window = self
                .players
                .get(&viewer)
                .and_then(|player| player.open.as_ref())
                .map(|menu| menu.window);
            if let Some(player) = self.players.get_mut(&viewer) {
                let rules = self.registry.item_rules();
                let effects = player.close(&rules);
                if let Some(connection) = connection {
                    for change in effects.changes {
                        self.push_player(connection, change);
                    }
                }
                for stack in effects.dropped {
                    let drop = spawn_item_entity(self, centre, stack);
                    spawned.push(drop);
                }
            }
            if let (Some(connection), Some(window)) = (connection, window) {
                self.push_player(connection, PlayerChange::MenuClosed { window });
            }
        }
        self.seq_counter.clear();
        // NOTE: `block_entity_removed` is NOT cleared here: `run_tick` drains
        // it after commit to emit `RemoveBlockEntity` tombstones in the same
        // tick's journal batch (see run_tick: drained via `std::mem::take`,
        // so no stale tombstones leak into later ticks).
        (deaths, events, spawned, removed)
    }

    fn player_tool_wear_plan(
        &self,
        actor: EntityId,
        amount: i32,
    ) -> Result<Option<crate::player::NativeToolWearPlan>, ()> {
        let Some(player) = self.players.get(&actor) else {
            return Ok(None);
        };
        let plan = player.plan_held_tool_wear(&self.registry.item_rules(), amount);
        if crate::plugin_inventories::below_player_debit_floors(
            self,
            actor,
            &[],
            plan.broken_slot(),
        ) {
            return Err(());
        }
        Ok(Some(plan))
    }

    fn apply_player_tool_wear(
        &mut self,
        actor: EntityId,
        plan: Option<crate::player::NativeToolWearPlan>,
    ) {
        if let Some(plan) = plan {
            let player = self
                .players
                .get_mut(&actor)
                .expect("prepared native tool owner");
            let connection = player.connection;
            let effects = player.apply_tool_wear(plan);
            for change in effects.changes {
                self.push_player(connection, change);
            }
        }
    }

    fn record_block_mined(&mut self, actor: EntityId, broken: u32) {
        if self.connections.by_entity.contains_key(&actor)
            && let Some(player) = self.players.get_mut(&actor)
            && !player.creative()
            && let Some(block) = self.registry.blocks.by_id(mc_world::BlockStateId(broken))
        {
            let rules = self.registry.item_rules();
            let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
            if !self
                .registry
                .mining
                .requires_correct_tool_for_drops(broken)
                .unwrap_or(false)
                || crate::block_drops::correct_tool(&rules, &block.block, held)
            {
                player
                    .statistics
                    .increment(StatType::Mined, block.block.raw_id as u32, 1);
            }
        }
    }

    fn block_break_tool_wear(&self, actor: EntityId) -> i32 {
        let Some(player) = self.players.get(&actor) else {
            return 0;
        };
        let rules = self.registry.item_rules();
        let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
        rules
            .facts
            .facts_for_stack(held, rules.items)
            // The native tool component defaults omitted damage_per_block to
            // one; only a missing tool component or explicit zero skips wear.
            .and_then(|facts| facts.tool.as_ref())
            .map_or(0, |tool| {
                i32::try_from(tool.damage_per_block.unwrap_or(1)).unwrap_or(i32::MAX)
            })
    }

    pub(crate) fn block_break_experience(&self, actor: EntityId, broken: u32) -> i32 {
        let Some(player) = self.players.get(&actor) else {
            return 0;
        };
        if player.creative() || !crate::command_rules::boolean(self, "block_drops") {
            return 0;
        }
        let rules = self.registry.item_rules();
        let held = &player.inventory.slots[usize::from(player.inventory.selected.min(8))];
        crate::block_drops::block_experience(
            &crate::block_drops::BlockDropRules {
                blocks: &self.registry.blocks,
                loot: &self.registry.loot,
                mining: &self.registry.mining,
            },
            &rules,
            broken,
            held,
            self.tick ^ actor.0 as u64,
        )
    }

    /// Drops and pre-wear experience for one accepted break. Spawns loot as item
    /// entities (vanilla pickup delay, small deterministic velocity).
    fn commit_break_drops(
        &mut self,
        actor: EntityId,
        pos: BlockPos,
        broken: u32,
        drops: Vec<mc_data::ItemStack>,
        xp: i32,
        spawned: &mut Vec<EntityId>,
    ) {
        let creative_shulker = self
            .players
            .get(&actor)
            .is_some_and(|player| player.creative())
            && self
                .registry
                .blocks
                .by_id(mc_world::BlockStateId(broken))
                .is_some_and(|block| crate::shulker::is_block(block.block.id.path()))
            && crate::block_drop_context::retains_contents(&drops);
        if creative_shulker {
            // Native playerWillDestroy spawns collected box components directly,
            // independent of the ordinary block-loot gamerule.
            let centre = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
            for stack in drops {
                spawned.push(spawn_item_entity(self, centre, stack));
            }
        } else {
            self.spawn_block_loot(pos, drops, actor.0 as u64, spawned);
        }

        let centre = [
            f64::from(pos.x) + 0.5,
            f64::from(pos.y) + 0.5,
            f64::from(pos.z) + 0.5,
        ];
        crate::experience_orbs::award(self, centre, xp, spawned);
    }

    /// Spawns block loot at the block centre as item entities (vanilla
    /// pickup delay, small deterministic scatter from `salt`).
    pub(crate) fn spawn_block_loot(
        &mut self,
        pos: BlockPos,
        drops: Vec<mc_data::ItemStack>,
        seed: u64,
        spawned: &mut Vec<EntityId>,
    ) {
        if !crate::command_rules::boolean(self, "block_drops") {
            return;
        }
        let centre = [pos.x as f64 + 0.5, pos.y as f64 + 0.5, pos.z as f64 + 0.5];
        for (index, stack) in drops.into_iter().enumerate() {
            let id = self.entities.alloc_id();
            let uuid = self.entity_uuid(id);
            let type_id = self.registry.item_type_id;
            // Deterministic scatter (no RNG on the tick path).
            let scatter = |salt: u64| {
                let hash = seed
                    .wrapping_mul(0xBF58_476D_1CE4_E5B9)
                    .wrapping_add(self.tick.wrapping_mul(0x9E37_79B9_7F4A_7C15))
                    .wrapping_add(salt)
                    .wrapping_add(index as u64);
                (hash as f64 / u64::MAX as f64 - 0.5) * 0.4
            };
            let appearance = std::sync::Arc::new(EntityAppearance {
                item: Some(stack.clone()),
                health: 1.0,
                ..Default::default()
            });
            self.entities.spawn(
                id,
                uuid,
                EntityKind::Item { type_id },
                ColdFields {
                    pos: centre,
                    vel: [scatter(1), 0.1, scatter(2)],
                    yaw: 0.0,
                    pitch: 0.0,
                    ground: false,
                    health: 1.0,
                    breed_cooldown: 0,
                    pickup_delay: 10,
                    item: Some(crate::entities::StackRow {
                        item_id: stack.item_id,
                        count: stack.count,
                        stack: Some(stack),
                    }),
                    appearance,
                    movement: crate::blueprint::MovementClass::Item,
                    half_width: 0.125,
                    height: 0.25,
                    speed: 0.0,
                    attack_damage: 0.0,
                    sight_range: 0.0,
                    attack_reach: 0.0,
                },
            );
            spawned.push(id);
        }
    }

    /// New buckets from a stacked bucket: into the inventory (vanilla
    /// `ItemUtils.createFilledResult`), the overflow drops at the player.
    fn commit_gives(&mut self, spawned: &mut Vec<EntityId>) {
        for (actor, item) in std::mem::take(&mut self.merge_gives) {
            let rules = self.registry.item_rules();
            let mut stack = mc_data::ItemStack::new(item, 1);
            let Some(player) = self.players.get_mut(&actor) else {
                continue;
            };
            player.inventory.add(&rules, &mut stack);
            let contents = player.inventory_contents();
            if let Some(connection) = self.connections.entity_connection(actor) {
                self.push_player(connection, contents);
            }
            if !stack.is_empty()
                && let Some(slot) = self.entities.slot_of(actor)
            {
                let pos = self.entities.pos(slot);
                let drop = spawn_item_entity(self, pos, stack);
                spawned.push(drop);
            }
        }
    }

    /// Loot of a block that popped or washed away (no tool).
    pub(crate) fn pop_block_loot(
        &mut self,
        pos: BlockPos,
        block: u32,
        spawned: &mut Vec<EntityId>,
    ) -> bool {
        let drops = if crate::command_rules::boolean(self, "block_drops") {
            crate::block_drop_context::roll(
                self,
                pos,
                block,
                &mc_data::ItemStack::EMPTY,
                self.tick ^ ((pos.x as u64) << 40) ^ ((pos.z as u64) << 16) ^ pos.y as u64,
            )
        } else {
            Ok(Vec::new())
        };
        let Some(drops) = crate::block_drop_context::admit(self, pos, drops) else {
            return false;
        };
        self.stage_container_removal(pos, !crate::block_drop_context::retains_contents(&drops));
        self.spawn_block_loot(pos, drops, pos.x as u64 ^ pos.z as u64, spawned);
        true
    }

    /// Remove an entity by slot; fixes connection/inventory/health maps.
    pub fn remove_entity(&mut self, slot: EntitySlot) {
        let body = self.entities.body(slot);
        let id = body.id;
        let kind = body.kind;
        let movement = body.movement;
        if self.entities.row(slot).projectile.is_some()
            || movement == crate::blueprint::MovementClass::Projectile
        {
            let uuid = self.entities.row(slot).uuid;
            crate::physical_attacks::drop_projectile(self, uuid);
        }
        self.armor_stand_hits.remove(&id);
        if let EntityKind::Player { .. } = kind
            && let Some(connection) = self.connections.by_entity.get(&id).copied()
        {
            self.connections.by_connection.remove(&connection);
            self.connections.by_entity.remove(&id);
            self.connections.view_distances.remove(&connection);
            self.connections.locales.remove(&connection);
            self.outbox.remove(&connection);
        }
        self.players.remove(&id);
        let last = self.entities.len() as u32 - 1;
        self.entities.remove(slot);
        self.follow_swap_removal(slot, last);
    }

    /// Swap-removal moved the row at `last` into `removed`. Slots staged for
    /// the merge/commit (damage, pickups, container custody) follow that row;
    /// entries for the removed row itself are dropped, so no staged slot can
    /// point past the store or at an unrelated entity.
    fn follow_swap_removal(&mut self, removed: EntitySlot, last: u32) {
        let follow = |slot: EntitySlot| {
            if slot == removed {
                None
            } else if slot.0 == last {
                Some(removed)
            } else {
                Some(slot)
            }
        };
        self.merge_damage.retain_mut(|(slot, _)| {
            follow(*slot).is_some_and(|next| {
                *slot = next;
                true
            })
        });
        self.plugin_damage.retain_mut(|(slot, _)| {
            follow(*slot).is_some_and(|next| {
                *slot = next;
                true
            })
        });
        self.merge_pickups.retain_mut(|(item, collector, _)| {
            let (Some(next_item), Some(next_collector)) = (follow(*item), follow(*collector))
            else {
                return false;
            };
            *item = next_item;
            *collector = next_collector;
            true
        });
        if !self.merge_consumed_items.is_empty() {
            self.merge_consumed_items = self
                .merge_consumed_items
                .drain()
                .filter_map(follow)
                .collect();
        }
    }

    /// Spawn a mob/item entity from a blueprint type id.
    pub fn spawn_mob(&mut self, type_id: u32, pos: [f64; 3], health: f32) -> EntityId {
        let id = self.entities.alloc_id();
        let uuid = self.entity_uuid(id);
        let blueprint = self.registry.blueprint(type_id);
        let (movement, half_width, height, speed, damage, sight, reach) = blueprint
            .map(|blueprint| {
                (
                    blueprint.movement,
                    blueprint.half_width,
                    blueprint.height,
                    blueprint.params.speed,
                    blueprint.params.attack_damage,
                    blueprint.sensors.sight_range,
                    blueprint.sensors.attack_reach,
                )
            })
            .unwrap_or((
                crate::blueprint::MovementClass::Walker,
                0.3,
                1.8,
                2.0,
                0.0,
                10.0,
                2.0,
            ));
        let is_item = type_id == self.registry.item_type_id;
        let kind = if is_item {
            EntityKind::Item { type_id }
        } else {
            EntityKind::Mob { type_id }
        };
        let mut appearance = EntityAppearance {
            health,
            ..Default::default()
        };
        if self
            .registry
            .blueprint(type_id)
            .is_some_and(|blueprint| blueprint.wool)
        {
            let roll = uuid.as_u128();
            let color = crate::blueprint::natural_wool_color(roll as u64, (roll >> 64) as u64);
            appearance.extra.push((
                crate::blueprint::SHEEP_WOOL_DATA_INDEX,
                crate::boundary::EntityDataValue::Byte(color as i8),
            ));
        }
        let appearance = Arc::new(appearance);
        self.entities.spawn(
            id,
            uuid,
            kind,
            ColdFields {
                pos,
                vel: [0.0; 3],
                yaw: 0.0,
                pitch: 0.0,
                ground: false,
                health,
                // Adults start without a cooldown; birth initializes child age
                // separately through the native lifecycle module.
                breed_cooldown: 0,
                pickup_delay: if is_item {
                    self.registry
                        .blueprint(type_id)
                        .map(|blueprint| blueprint.params.pickup_delay)
                        .unwrap_or(10)
                } else {
                    0
                },
                item: None,
                appearance,
                movement,
                half_width,
                height,
                speed,
                attack_damage: damage,
                sight_range: sight,
                attack_reach: reach,
            },
        );
        if let Some(slot) = self.entities.slot_of(id) {
            crate::villager::conversion::initialize(self, slot, type_id);
            crate::entity_loot::initialize(self, slot);
            crate::tnt::initialize(self, slot, type_id);
            crate::falling::initialize(self, slot, type_id);
            crate::vehicles::initialize(self, slot);
            crate::special_mobs::initialize(self, slot, type_id);
            crate::bees::initialize(self, slot, type_id);
            crate::allay::initialize(self, slot, type_id);
            crate::copper_golem::initialize(self, slot, type_id);
            crate::mob_lanes::initialize(self, slot, type_id);
            crate::leash::initialize(self, slot);
            crate::end_fight::initialize_entity(self, slot);
            crate::dolphin::initialize(self, slot, type_id);
            crate::mob_equipment::initialize(self, slot, type_id);
        }
        id
    }

    /// Enter at the simulation radius; retain the previous set one chunk
    /// farther out so crossing a chunk boundary does not flap simulation.
    pub(crate) fn refresh_active_chunks(&mut self, tick: u64) {
        let radius = self.config_sim_distance.max(0);
        let entities = &self.entities;
        let centers = self.players.keys().filter_map(|id| {
            let pos = entities.pos(entities.slot_of(*id)?);
            Some(ChunkPos {
                x: (mc_physics::mth::floor_i32(pos[0])).div_euclid(16),
                z: (mc_physics::mth::floor_i32(pos[2])).div_euclid(16),
            })
        });
        if radius == self.active_sim_radius
            && centers
                .clone()
                .eq(self.active_player_centers.iter().copied())
            && self
                .ticket_areas()
                .map(|(center, _, radius)| (center, radius))
                .eq(self.active_ticket_ranges.iter().copied())
        {
            self.world.refresh_active_clock(tick);
            return;
        }
        self.active_player_centers.clear();
        self.active_player_centers.extend(centers);
        let ranges: Vec<_> = self
            .ticket_areas()
            .map(|(center, _, radius)| (center, radius))
            .collect();
        self.active_ticket_ranges = ranges;
        self.active_sim_radius = radius;
        let mut active = HashSet::default();
        for center in &self.active_player_centers {
            for z in center.z - radius..=center.z + radius {
                for x in center.x - radius..=center.x + radius {
                    active.insert(ChunkPos { x, z });
                }
            }
            for chunk in self.world.active_positions() {
                if (i64::from(chunk.x) - i64::from(center.x)).abs() <= i64::from(radius) + 1
                    && (i64::from(chunk.z) - i64::from(center.z)).abs() <= i64::from(radius) + 1
                {
                    active.insert(chunk);
                }
            }
        }
        for (center, _, radius) in self.ticket_areas() {
            for z in center.z - radius..=center.z + radius {
                for x in center.x - radius..=center.x + radius {
                    active.insert(ChunkPos { x, z });
                }
            }
        }
        self.world.set_active_chunks(active, tick);
    }

    /// Vanilla's spawning/ticking chunks: resident simulation chunks whose
    /// centres are less than 128 blocks from a non-spectator player.
    fn accumulate_inhabited_time(&mut self) {
        // Accumulation precedes commit's tick increment; interval boundaries
        // must use the output/WAL tick, including the first tick after recovery.
        let tick = self.tick + 1;
        // Forced chunks within the spawning sphere count too, independently
        // of the player's configured simulation radius.
        let radius = 8;
        for (id, player) in &self.players {
            if player.spectator() {
                continue;
            }
            let Some(slot) = self.entities.slot_of(*id) else {
                continue;
            };
            let pos = self.entities.pos(slot);
            let cx = (mc_physics::mth::floor_i32(pos[0])).div_euclid(16);
            let cz = (mc_physics::mth::floor_i32(pos[2])).div_euclid(16);
            for z in cz - radius..=cz + radius {
                for x in cx - radius..=cx + radius {
                    let dx = f64::from(x) * 16.0 + 8.0 - pos[0];
                    let dz = f64::from(z) * 16.0 + 8.0 - pos[2];
                    if dx * dx + dz * dz < 128.0 * 128.0 && self.world.is_active(x, z) {
                        self.world.tick_inhabited_time(ChunkPos { x, z }, tick);
                    }
                }
            }
        }
        self.world.finish_inhabited_tick(tick);
    }

    /// The sole residency demand: per-player effective view (negotiated client
    /// radius, capped at the configured view, floored by the simulation
    /// radius) plus one chunk of margin, active chunks, explicit/respawn
    /// tickets and unacknowledged dirty pins. Unnegotiated players use the
    /// configured view. Load priorities prefer the nearest player/ticket
    /// centre; demands union across players.
    pub fn desired_chunks(&self) -> HashMap<ChunkPos, i32> {
        let mut want: HashMap<ChunkPos, i32> = HashMap::default();
        let mut add = |center: ChunkPos, radius: i32| {
            for z in center.z - radius..=center.z + radius {
                for x in center.x - radius..=center.x + radius {
                    let (dx, dz) = (x - center.x, z - center.z);
                    let priority = dx * dx + dz * dz;
                    want.entry(ChunkPos { x, z })
                        .and_modify(|best| *best = (*best).min(priority))
                        .or_insert(priority);
                }
            }
        };
        for id in self.players.keys() {
            let Some(slot) = self.entities.slot_of_any(*id) else {
                continue;
            };
            let pos = self.entities.pos(slot);
            let center = ChunkPos {
                x: (mc_physics::mth::floor_i32(pos[0])).div_euclid(16),
                z: (mc_physics::mth::floor_i32(pos[2])).div_euclid(16),
            };
            add(center, self.residency_radius_for(*id));
            if !self.entities.body(slot).alive {
                let spawn = self.world.world_spawn();
                add(
                    ChunkPos {
                        x: spawn[0].div_euclid(16),
                        z: spawn[2].div_euclid(16),
                    },
                    1,
                );
                if let Some(spawn) = self.players.get(id).and_then(|player| player.respawn_point) {
                    add(
                        ChunkPos {
                            x: spawn[0].div_euclid(16),
                            z: spawn[2].div_euclid(16),
                        },
                        1,
                    );
                }
            }
        }
        for (centre, load, _) in self.ticket_areas() {
            add(centre, load);
        }
        for chunk in self.world.active_positions().chain(self.world.dirty_pins()) {
            want.entry(chunk).or_insert(i32::MAX);
        }
        want
    }

    /// Effective residency radius for one joined player: negotiated client
    /// view capped at the configured view, floored by the configured
    /// simulation radius, plus one chunk of margin. Unnegotiated players
    /// use the configured view (so `max` keeps at least simulation).
    fn residency_radius_for(&self, id: EntityId) -> i32 {
        let negotiated = self
            .connections
            .entity_connection(id)
            .and_then(|connection| self.connections.view_distance_of(connection))
            .map(i32::from);
        let effective_view = match negotiated {
            Some(view) => view.min(self.config_view_distance.max(0)),
            None => self.config_view_distance.max(0),
        }
        .max(self.config_sim_distance.max(0));
        effective_view.saturating_add(1)
    }
}

/// Wool byte (color id, `0x10` sheared) of a wool-growing mob; white when
/// unset.
pub(crate) fn wool_of(appearance: &EntityAppearance) -> u8 {
    appearance
        .extra
        .iter()
        .find_map(|(index, value)| match value {
            crate::boundary::EntityDataValue::Byte(byte)
                if *index == crate::blueprint::SHEEP_WOOL_DATA_INDEX =>
            {
                Some(*byte as u8)
            }
            _ => None,
        })
        .unwrap_or(0)
}

/// Writes the wool byte of a wool-growing mob.
pub(crate) fn set_wool(appearance: &mut EntityAppearance, wool: u8) {
    let value = crate::boundary::EntityDataValue::Byte(wool as i8);
    match appearance
        .extra
        .iter_mut()
        .find(|(index, _)| *index == crate::blueprint::SHEEP_WOOL_DATA_INDEX)
    {
        Some(entry) => entry.1 = value,
        None => appearance
            .extra
            .push((crate::blueprint::SHEEP_WOOL_DATA_INDEX, value)),
    }
}

/// Block sampler over the tick's read state for `mc-physics`.
pub(crate) struct TickSampler<'a> {
    pub(crate) world: &'a ChunkWorld,
    pub(crate) registry: &'a CoreRegistry,
}

impl mc_physics::BlockSampler for TickSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        match self.world.physics_block_at(x, y, z) {
            Some(state) => self.registry.materials.classify(state),
            // Missing chunk blocks movement: never a floor, never passable.
            None => mc_physics::BlockMaterial::Solid,
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        self.registry.max_collision_box_y
    }

    fn stationary_downward_clip(
        &self,
        body: mc_physics::EntityBody,
        displacement_y: f64,
        max_scan_cells: usize,
        max_candidates: usize,
    ) -> Option<f64> {
        mc_physics::stationary_support_clip(
            self,
            body,
            displacement_y,
            max_scan_cells,
            max_candidates,
            self.registry.max_collision_boxes_per_cell,
        )
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        emit_collision_boxes(self.registry, self.world.physics_block_at(x, y, z), emit);
    }
}

/// Worker-thread sampler for the entity phase: missing columns resolve as
/// [`BlockMaterial::Solid`] so unloaded chunks block motion and never form
/// a floor. Shares the tick's read state by reference (no locks, no copies).
struct WorkerSampler<'a> {
    block_at: &'a dyn Fn(i32, i32, i32) -> Option<u32>,
    registry: &'a CoreRegistry,
}

impl mc_physics::BlockSampler for WorkerSampler<'_> {
    fn material_at(&self, x: i32, y: i32, z: i32) -> mc_physics::BlockMaterial {
        match (self.block_at)(x, y, z) {
            Some(state) => self.registry.materials.classify(state),
            None => mc_physics::BlockMaterial::Solid,
        }
    }

    fn max_collision_box_y(&self) -> u8 {
        self.registry.max_collision_box_y
    }

    fn stationary_downward_clip(
        &self,
        body: mc_physics::EntityBody,
        displacement_y: f64,
        max_scan_cells: usize,
        max_candidates: usize,
    ) -> Option<f64> {
        mc_physics::stationary_support_clip(
            self,
            body,
            displacement_y,
            max_scan_cells,
            max_candidates,
            self.registry.max_collision_boxes_per_cell,
        )
    }

    fn collision_boxes_at(
        &self,
        x: i32,
        y: i32,
        z: i32,
        emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
    ) {
        emit_collision_boxes(self.registry, (self.block_at)(x, y, z), emit);
    }
}

fn emit_collision_boxes(
    registry: &CoreRegistry,
    state: Option<u32>,
    emit: &mut dyn FnMut(mc_physics::BlockCollisionBox),
) {
    if let Some(boxes) = state.and_then(|state| registry.collision_boxes.get(state as usize)) {
        for &collision_box in boxes {
            emit(collision_box);
        }
    } else {
        // Unknown states and missing chunks remain conservative obstacles.
        emit(mc_physics::BlockCollisionBox::FULL_BLOCK);
    }
}

#[cfg(test)]
#[path = "tick_collision_tests.rs"]
mod collision_sampler_tests;

/// Build a [`PlayerState`] from an offline record (rejoin) or defaults.
/// Pure data hydration: inventory slots, vitals, xp, game mode.
fn hydrate_player(
    connection: ConnectionId,
    offline: Option<&crate::journal::ReplayPlayer>,
) -> crate::player::PlayerState {
    let mut player =
        crate::player::PlayerState::new(connection, crate::commands::GameMode::Survival);
    let Some(record) = offline else {
        return player;
    };
    player
        .opaque_player_fields
        .clone_from(&record.opaque_player_fields);
    player.statistics.clone_from(&record.statistics);
    player
        .advancement_progress
        .clone_from(&record.advancement_progress);
    player.unlocked_recipes.clone_from(&record.unlocked_recipes);
    if record.inventory.len() == player.inventory.slots.len() {
        player.inventory.slots = record.inventory.clone();
    }
    player.inventory.selected = record.selected.min(8);
    player.vitals.health = record.health;
    player.vitals.food = record.food;
    player.vitals.saturation = record.saturation;
    player.vitals.exhaustion = record.exhaustion;
    player.vitals.air = record.air;
    player.vitals.experience.level = record.experience_level;
    player.vitals.experience.progress = record.experience_progress;
    player.vitals.experience.total = record.experience_total;
    player.respawn_point = record.respawn;
    player.respawn_dimension = record.respawn_dimension;
    player.respawn_forced = record.respawn_forced;
    player.respawn_yaw = record.respawn_yaw;
    player.respawn_pitch = record.respawn_pitch;
    player.active_effects = record.active_effects.clone();
    player.current_impulse_impact_pos = record.current_impulse_impact_pos;
    player.fall_flying = record.fall_flying;
    player.warden_warning = record.warden_warning;
    if record.enchantment_seed != 0 {
        player.enchantment_seed = record.enchantment_seed;
    }
    player.game_mode = match record.game_mode {
        1 => crate::commands::GameMode::Creative,
        2 => crate::commands::GameMode::Adventure,
        3 => crate::commands::GameMode::Spectator,
        _ => crate::commands::GameMode::Survival,
    };
    player
}

/// Run one full tick: inputs -> entity phase -> merge -> block phase ->
/// commit -> publish. Returns the [`TickOutput`] plus per-connection
/// reliable outbox drains and the phase times (`total_us` is filled by the
/// tick loop).
pub fn run_tick(
    state: &mut TickState,
    inputs: &[PlayerInput],
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let wrapper_start = std::time::Instant::now();
    let phase_start = std::env::var_os("MC_CORE_PHASE_LOG")
        .is_some()
        .then(std::time::Instant::now);
    crate::plugin::pump(state);
    crate::tick_control::begin_tick(state);
    let pump_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    let dimensions_start = std::time::Instant::now();
    let mut result = crate::dimensions::run_tick(state, inputs);
    let dimensions_time = dimensions_start.elapsed();
    let wrapper_tail_start = std::time::Instant::now();
    let dimensions_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    crate::build_precommit::answer_previews(state);
    let previews_done = phase_start.map(|start| start.elapsed()).unwrap_or_default();
    crate::plugin::observe_players(state);
    result.2.plugins_us = dimensions_start.duration_since(wrapper_start).as_micros() as u64
        + wrapper_tail_start.elapsed().as_micros() as u64;
    // This envelope includes routing/travel/globals and destruction of local
    // phase scratch after the dimension function returns. It is not disk I/O
    // or edge network fanout.
    let local_us = result.2.input_us
        + result.2.chunks_us
        + result.2.entity_us
        + result.2.merge_us
        + result.2.block_us
        + result.2.commit_us
        + result.2.snapshot_us
        + result.2.views_us
        + result.2.publish_us;
    result.2.coordination_us = (dimensions_time.as_micros() as u64).saturating_sub(local_us);
    if let Some(start) = phase_start {
        let total = start.elapsed();
        eprintln!(
            "tick wrapper ms: tick={} pump={:.2} dimensions={:.2} previews={:.2} observe={:.2} total={:.2}",
            state.tick,
            pump_done.as_secs_f64() * 1000.0,
            dimensions_done.saturating_sub(pump_done).as_secs_f64() * 1000.0,
            previews_done.saturating_sub(dimensions_done).as_secs_f64() * 1000.0,
            total.saturating_sub(previews_done).as_secs_f64() * 1000.0,
            total.as_secs_f64() * 1000.0,
        );
    }
    result
}

struct DimensionPhaseLog {
    start: std::time::Instant,
    before_return: std::time::Duration,
    logging: std::time::Duration,
    allocator_us: u64,
}

/// One local world's phases; the dimension coordinator owns the global tick.
pub(crate) fn run_dimension_tick<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let mut phase_log =
        std::env::var_os("MC_CORE_PHASE_LOG")
            .is_some()
            .then(|| DimensionPhaseLog {
                start: std::time::Instant::now(),
                before_return: std::time::Duration::ZERO,
                logging: std::time::Duration::ZERO,
                allocator_us: 0,
            });
    let result = run_dimension_tick_inner(state, inputs, phase_log.as_mut());
    if let Some(log) = phase_log {
        // The inner call has returned, so this includes destruction of every
        // phase-local allocation, unlike the phase print inside that call.
        let total = log.start.elapsed();
        eprintln!(
            "dimension envelope ms: tick={} dimension={:?} total={:.2} return_cleanup={:.2} phase_log={:.2}",
            state.tick,
            state.dimension,
            total.as_secs_f64() * 1000.0,
            total
                .saturating_sub(log.before_return)
                .saturating_sub(log.logging)
                .as_secs_f64()
                * 1000.0,
            log.logging.as_secs_f64() * 1000.0,
        );
    }
    result
}

fn run_dimension_tick_inner<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
    mut phase_log: Option<&mut DimensionPhaseLog>,
) -> (
    TickOutput,
    Vec<(ConnectionId, Vec<PlayerChange>)>,
    TickTimes,
) {
    let input_start = std::time::Instant::now();
    // False while `/tick freeze` holds the world: only players, input, chunk
    // loading and merge/commit run; see `tick_control`.
    let sim = state.tick_control.runs_normally();
    let previous_weather = crate::weather::view(state);
    for player in state.players.values_mut() {
        player.statistics.advance_tick(state.tick);
        player.chat_spam_ticks = player.chat_spam_ticks.saturating_sub(1);
    }
    crate::chunk_tickets::refresh(state);
    state.refresh_active_chunks(state.tick + 1);
    // 1. Natural spawns/despawns, then inputs: player join/leave/move/dig/
    // attack/chat; intents staged.
    let mut intents: Vec<Intent> = Vec::new();
    let mut changes = ChangeSet::default();
    if sim {
        crate::natural_spawn::advance_ages_and_run(state, &mut changes);
        crate::mob_lanes::traders(state, &mut changes);
        crate::vibration::tick(state);
        crate::plugin_npcs::advance(state);
    }
    changes.entities_spawned.append(&mut state.plugin_spawned);
    state
        .merge_block_writes
        .append(&mut state.plugin_block_writes);
    let deferred = std::mem::take(&mut state.deferred_uses);
    // MC-28289: snapshot each player's held main-hand stack BEFORE this
    // tick's inputs run, so the melee path resolves attack_damage /
    // attack_speed from the pre-tick item when a swap+attack share a tick.
    for player in state.players.values_mut() {
        let held = player
            .inventory
            .slots
            .get(usize::from(player.inventory.selected.min(8)))
            .cloned()
            .unwrap_or(mc_data::ItemStack::EMPTY);
        player.attribute_snapshot_held = Some(held);
    }
    mining::advance(state);
    apply_inputs(state, &deferred, &mut intents, &mut changes);
    apply_inputs(state, inputs, &mut intents, &mut changes);
    state.refresh_active_chunks(state.tick + 1);
    crate::physical_attacks::advance(state, &mut intents);
    if sim {
        crate::shrieker::run(state);
    }
    tick_players(state, &mut changes, &mut intents);
    if sim {
        crate::raids::raid_omens(state);
    }
    crate::effects::run(state, &mut intents);
    crate::contact_hazards::run(state, &mut intents);
    crate::dimensions::rules::void_intents(state, &mut intents);
    crate::enchantment_locations::run(state, &mut intents);
    if sim {
        crate::end_fight::prepare(state);
    }
    let input_time = input_start.elapsed();

    // Desired chunks from players + tickets; load/unload.
    let chunk_start = std::time::Instant::now();
    let want = state.desired_chunks();
    let loaded = state.world.ensure(&want);
    state.refresh_active_chunks(state.tick + 1);
    state.accumulate_inhabited_time();
    // Inputs can mutate a resident-inactive container immediately before it
    // leaves demand. Fence its old time before eviction can forget authority.
    crate::abstract_sim::cancel_external_writes(state);
    for &(pos, _) in &state.merge_block_writes {
        state.world.cancel_simulation_debt(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        });
    }
    let unloaded = state.world.evict_outside(&want);
    state
        .block_ticks
        .evict_flushed(|x, z| state.world.unloaded_clean(x, z));
    state
        .fluid_ticks
        .evict_flushed(|x, z| state.world.unloaded_clean(x, z));
    crate::fluids::hydrate(state, &loaded);
    crate::villager::generated::load(state, &loaded, &mut changes);
    changes.chunks_loaded.extend(loaded);
    changes.chunks_unloaded.extend(unloaded);
    // Block entities ride inside chunk regions: pull newly resident chunks'
    // typed maps into live storage (journal folds apply over them).
    sync_chunk_block_entities(state);
    if sim {
        crate::settlements::autonomy::tick(state, &mut changes);
        crate::conduit::run(state, &mut intents);
        crate::beacon::run(state);
        crate::dolphin::run(state);
    }
    // Immediate respawn retries after off-tick chunk loading; unavailable
    // personal spawn chunks must never turn into a fallback-world respawn.
    if crate::command_rules::boolean(state, "immediate_respawn") {
        for index in 0..state.entities.len() as u32 {
            let body = state.entities.body(EntitySlot(index));
            if !body.alive
                && let EntityKind::Player { connection } = body.kind
            {
                respawn_player(state, connection, &mut changes);
            }
        }
    }
    let chunk_time = chunk_start.elapsed();

    // 2. Entity phase (parallel by data over the fixed pool).
    let entity_start = std::time::Instant::now();
    if sim {
        crate::entity_life::advance(state);
        crate::piglin::run(state, &mut changes);
        crate::tnt::run(state, &mut intents);
        crate::mob_lanes::snowballs(state);
    }
    let entity_intents = run_entity_phase(state, sim);
    let entity_time = entity_start.elapsed();
    let entity_tail_start = std::time::Instant::now();
    intents.extend(entity_intents);
    if sim {
        crate::special_mobs::raiders::run(state, &mut intents);
    }
    crate::vehicles::run(state, &mut intents, &mut changes);
    if sim {
        crate::end_fight::run(state, &mut intents, &mut changes);
        crate::villager::soldier::attacks(state, &mut intents, &mut changes);
    }
    let entity_tail_time = entity_tail_start.elapsed();

    // 3. Merge (deterministic, fixed order, whole-intent rejection).
    let merge_start = std::time::Instant::now();
    state.merge(&mut intents);
    state.merge_damage.append(&mut state.plugin_damage);
    crate::end_fight::crystal_hits(state);
    let merge_time = merge_start.elapsed();
    let block_prepare_start = std::time::Instant::now();

    // 4. Block phase: ordered merge writes, retained falling lifecycle.
    let mut block_writes = std::mem::take(&mut state.block_phase_writes);
    block_writes.extend_from_slice(&state.merge_block_writes);
    let early_merge_writes = state.merge_block_writes.len();
    let block_prepare_time = block_prepare_start.elapsed();
    let block_start = std::time::Instant::now();
    for (pos, write_state) in &block_writes {
        state.world.set_block(pos.x, pos.y, pos.z, *write_state);
    }
    // Single merge owner: accepted writes win over old chunk time. Catch-up
    // writes are appended only after normal staged writes have been applied.
    crate::abstract_sim::cancel_external_writes(state);
    crate::abstract_sim::run(state, &mut block_writes);
    // Attached blocks whose support went away pop off with their loot.
    let mut popped_drops = Vec::new();
    pop_unsupported(state, &mut block_writes, &mut popped_drops);
    if sim {
        // Random ticks: crops, farmland, saplings, grass.
        crate::farming::run(state, &mut block_writes, &mut popped_drops);
        // Due water/lava ticks, then schedule the fluid around every write.
        crate::fluids::run(state, &mut block_writes, &mut popped_drops);
        // Furnace cooking over live block-entity storage (input/fuel/output).
        tick_furnaces(state, &mut block_writes, None);
        crate::sculk::run(state, &mut block_writes);
        crate::crafter::advance(state, &mut block_writes);
        crate::redstone::run(state, &mut block_writes, &mut popped_drops);
        crate::jukebox::run(state, &mut block_writes, &mut popped_drops);
        crate::brewing::run(state, &mut block_writes, &mut popped_drops);
        crate::crafter::run(state);
    }
    crate::shelf::run(state);
    crate::shulker::run(state);
    // Scheduled ignition and dispenser actions prime after the early merge
    // writes were copied. Their air post-images must commit in this same tick.
    for &(pos, write_state) in &state.merge_block_writes[early_merge_writes..] {
        state.world.set_block(pos.x, pos.y, pos.z, write_state);
        block_writes.push((pos, write_state));
    }
    crate::blocks::run(
        state,
        &mut block_writes,
        &mut popped_drops,
        &mut changes.entities_removed,
    );
    if sim {
        crate::special_mobs::after_blocks(state, &mut block_writes);
        crate::bees::run(state, &mut block_writes, &mut changes);
    }
    if sim {
        crate::copper_golem::after_blocks(state, &mut block_writes);
    }
    crate::golems::after_blocks(state, &mut block_writes);
    if sim {
        crate::copper_golem::tick(state, &mut block_writes);
        crate::mob_lanes::snow_trails(state, &mut block_writes);
    }
    crate::portals::nether::after_blocks(state, &mut block_writes);
    crate::fluids::schedule(state, &block_writes);
    let block_time = block_start.elapsed();

    // 5. Commit: fold blocks, apply entity effects.
    let commit_start = std::time::Instant::now();
    state.world.fold_overlay();
    crate::shrieker::after_blocks(state, &block_writes);
    crate::end_portals::after_blocks(state, &block_writes);
    crate::conduit::after_blocks(state, &block_writes);
    crate::campfire::run(state, &mut popped_drops);
    let commit_write_start = state.merge_block_writes.len();
    let (deaths, mut events, spawned_drops, committed_removals) = state.commit_merge_effects();
    block_writes.extend_from_slice(&state.merge_block_writes[commit_write_start..]);
    if state.merge_block_writes.len() != commit_write_start {
        state.world.fold_overlay();
    }
    crate::shulker::commit_placements(state);
    crate::block_entity_defaults::after_blocks(state, &block_writes);
    crate::trial_spawner::after_blocks(state, &block_writes);
    crate::vault::after_blocks(state, &block_writes);
    crate::block_log::after_blocks(state);
    if sim {
        crate::trial_spawner::run(state, &mut block_writes, &mut changes);
        crate::vault::run(state, &mut block_writes, &mut changes);
    }
    changes.entities_spawned.extend(spawned_drops);
    changes.entities_spawned.extend(popped_drops);
    changes.entities_spawned.append(&mut state.hostile.spawned);
    changes
        .entities_spawned
        .append(&mut state.projectiles.spawned);
    if sim {
        crate::experience_orbs::run(state, &mut changes, &mut events);
    }
    changes.explosions.append(&mut state.hostile.explosions);
    events.append(&mut state.hostile.events);
    changes.entities_removed.append(&mut state.hostile.removed);
    changes
        .entities_removed
        .append(&mut state.projectiles.removed);
    // Committed full-pickup removals journal their tombstones in this same
    // tick (durable pickup: credit + tombstone share one batch).
    changes.entities_removed.extend(committed_removals);
    state.merge_block_writes.clear();
    if sim {
        crate::bees::commit(state, &mut changes);
        crate::villager::conversion::tick(state);
    }
    // Settlements allocator: sole POI authority, immediately before the brain,
    // sharing the global tick budget with its own telemetry phase.
    let allocator_us = if sim {
        let allocator_us = crate::settlements::tick(state, &mut changes);
        crate::villager::brain::tick(state, &mut changes);
        allocator_us
    } else {
        0
    };
    if let Some(log) = phase_log.as_mut() {
        log.allocator_us += allocator_us;
    }
    if sim {
        merchant::maintain(state, &mut changes);
        crate::mounts::tick(state, &mut changes);
        crate::allay::run(state, &mut changes);
        crate::mob_equipment_pickup::run(state, &mut changes);
    }
    crate::entity_life::commit_drops(state, &mut changes.entities_spawned);
    if sim {
        crate::end_fight::commit(state, &mut changes);
        crate::raids::run(state, &mut changes);
    }

    // NOTE: Health(0) + Died emit inside commit_merge_effects (death-screen
    // order, same tick as the drops) — no second emission here; commit is
    // the single owner of the death outbox sequence.
    if crate::command_rules::boolean(state, "immediate_respawn") {
        for entity in deaths {
            if let Some(connection) = state.connections.entity_connection(entity) {
                respawn_player(state, connection, &mut changes);
            }
        }
    }

    state.tick += 1;
    crate::statistics::tick(state, state.tick);
    if sim {
        crate::end_portals::tick(state);
    }
    if crate::tick_control::advances_day(state) {
        state.day_time = state.day_time.wrapping_add(1);
    }
    if !sim {
        // Frozen: the weather clock, sleep and fades hold with the world.
    } else if state.dimension.has_weather() {
        crate::weather::advance_cycle(state);
        crate::sleep::run(state);
        crate::weather::fade_and_publish(state, previous_weather);
    } else {
        state.weather = (crate::commands::Weather::Clear, 0);
        state.rain_level = 0.0;
        state.thunder_level = 0.0;
    }
    crate::scoreboard::automatic(state, &[], &[]);
    crate::command_locate::drain(state);
    changes
        .blocks
        .extend(
            block_writes
                .iter()
                .map(|(pos, write_state)| crate::boundary::BlockChange {
                    position: *pos,
                    state: *write_state,
                }),
        );
    block_writes.clear();
    state.block_phase_writes = block_writes;
    // Player inventory is the equipment authority. Project changes through
    // the same appearance path used by all living entities/SetEquipment.
    for (&entity, player) in &state.players {
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        let slots = [
            usize::from(player.inventory.selected.min(8)),
            crate::inventory::OFF_HAND,
            36,
            37,
            38,
            39,
        ];
        let row = state.entities.row_mut(slot);
        let mut changed = false;
        for (index, inventory_slot) in slots.into_iter().enumerate() {
            let stack = &player.inventory.slots[inventory_slot];
            let expected = (!stack.is_empty()).then_some(stack);
            if row.appearance.equipment[index].as_ref() != expected {
                Arc::make_mut(&mut row.appearance).equipment[index] = expected.cloned();
                changed = true;
            }
        }
        if changed {
            state.appearance_changed.push(entity);
        }
    }
    changes.entity_events.append(&mut events);
    changes
        .entities_appearance_changed
        .append(&mut state.appearance_changed);

    let attribute_changes: Vec<_> = state
        .players
        .iter_mut()
        .filter_map(|(&entity, player)| {
            let connection = state.connections.entity_connection(entity)?;
            player
                .equipment_attributes(&state.registry.item_rules())
                .map(|change| (connection, change))
        })
        .collect();
    for (connection, change) in attribute_changes {
        state.push_player(connection, change);
    }
    crate::advancements::run(state, &mut changes);
    changes.entities_spawned.append(&mut state.plugin_spawned);
    let plugin_events = crate::plugin_player::tick(state);
    if let Some(runtime) = &mut state.plugin_runtime {
        runtime.pending_events.extend(plugin_events);
    }
    let commit_time = commit_start.elapsed();
    // 6. Publish: chunk list + entity wire views + per-connection outbox
    // drain. Readers hold the snapshot `Arc`; unchanged chunks share theirs.
    let publish_start = std::time::Instant::now();
    // Native commit-tail owners (including villager work and doors) can write
    // after the main block fold. Publish their actual chunk afterimages, not
    // an older chunk beside newer block deltas.
    // Canonical structure work may run in the resident commit tail, after
    // the early plugin-write drain. Seal those writes with debit/cursor now,
    // using final owner post-images (a door/shape may have evolved meanwhile).
    changes
        .blocks
        .extend(state.plugin_block_writes.drain(..).map(|(position, _)| {
            crate::boundary::BlockChange {
                position,
                state: state
                    .world
                    .block_at(position.x, position.y, position.z)
                    .expect("committed native block remains loaded through tick seal"),
            }
        }));
    state.world.fold_overlay();
    let chunks = state.world.snapshot();
    let snapshot_time = publish_start.elapsed();
    let views_start = std::time::Instant::now();
    let views = state.entities.views();
    let views_time = views_start.elapsed();
    let snapshot = Arc::new(WorldSnapshot {
        tick: state.tick,
        dimension: state.dimension,
        dimensions: Default::default(),
        day_time: state.day_time,
        day_time_rate: if crate::command_rules::boolean(state, "advance_time") {
            1.0
        } else {
            0.0
        },
        raining: state.weather.0 != crate::commands::Weather::Clear,
        rain_level: state.rain_level,
        thunder_level: state.thunder_level,
        border: state.border,
        chunks,
        entities: views,
        raid_bars: crate::raids::bars(state),
        settlement_labels: crate::settlements::presentation::snapshot(state),
    });
    let output = TickOutput {
        tick: state.tick,
        snapshot,
        changes: Arc::new(changes),
        times: TickTimes::default(),
    };
    let mut drains = Vec::new();
    let connections: Vec<ConnectionId> = state.outbox.keys().copied().collect();
    for connection in connections {
        if let Some(queue) = state.outbox.remove(&connection)
            && !queue.is_empty()
        {
            drains.push((connection, queue));
        }
    }
    let publish_tail_time = publish_start
        .elapsed()
        .saturating_sub(snapshot_time)
        .saturating_sub(views_time);
    if let Some(log) = phase_log {
        log.before_return = log.start.elapsed();
        let measured = input_time
            + chunk_time
            + entity_time
            + entity_tail_time
            + merge_time
            + block_prepare_time
            + block_time
            + commit_time
            + snapshot_time
            + views_time
            + publish_tail_time;
        let log_start = std::time::Instant::now();
        eprintln!(
            "phase ms: tick={} input={:.2} chunks={:.2} entity={:.2} merge={:.2} block={:.2} commit={:.2} snapshot={:.2} views={:.2} intents={} dimension={:?} entity_tail={:.2} block_prepare={:.2} publish_tail={:.2} boundary_gaps={:.2} before_return={:.2} allocator={:.2}",
            state.tick,
            input_time.as_secs_f64() * 1000.0,
            chunk_time.as_secs_f64() * 1000.0,
            entity_time.as_secs_f64() * 1000.0,
            merge_time.as_secs_f64() * 1000.0,
            block_time.as_secs_f64() * 1000.0,
            commit_time.as_secs_f64() * 1000.0,
            snapshot_time.as_secs_f64() * 1000.0,
            views_time.as_secs_f64() * 1000.0,
            intents.len(),
            state.dimension,
            entity_tail_time.as_secs_f64() * 1000.0,
            block_prepare_time.as_secs_f64() * 1000.0,
            publish_tail_time.as_secs_f64() * 1000.0,
            log.before_return.saturating_sub(measured).as_secs_f64() * 1000.0,
            log.before_return.as_secs_f64() * 1000.0,
            log.allocator_us as f64 / 1000.0,
        );
        log.logging = log_start.elapsed();
    }
    let times = TickTimes {
        input_us: input_time.as_micros() as u64,
        chunks_us: chunk_time.as_micros() as u64,
        entity_us: (entity_time + entity_tail_time).as_micros() as u64,
        merge_us: merge_time.as_micros() as u64,
        block_us: (block_prepare_time + block_time).as_micros() as u64,
        commit_us: commit_time.as_micros() as u64,
        snapshot_us: snapshot_time.as_micros() as u64,
        views_us: views_time.as_micros() as u64,
        publish_us: publish_tail_time.as_micros() as u64,
        total_us: 0,
        ..Default::default()
    };
    (output, drains, times)
}

/// Apply one [`PlayerEffects`](crate::player::PlayerEffects): reliable
/// changes to the connection, drops as item-spawn intents (merge path).
pub(crate) fn apply_player_effects(
    state: &mut TickState,
    connection: ConnectionId,
    entity: EntityId,
    effects: crate::player::PlayerEffects,
    changes: &mut ChangeSet,
) {
    for change in effects.changes {
        state.push_player(connection, change);
    }
    for (recipe, ingredients) in effects.crafted_recipes {
        let ingredients: Vec<_> = ingredients
            .iter()
            .filter(|item| !item.is_empty())
            .map(|item| crate::advancements::item_snapshot(state, item))
            .collect();
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:recipe_crafted",
            serde_json::json!({"recipe_id":recipe,"ingredients":ingredients}),
        );
    }
    if let Some(item) = &effects.consumed_item {
        let item = crate::advancements::item_snapshot(state, item);
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:consume_item",
            serde_json::json!({"item":item}),
        );
    }
    for potion in effects.brewed_potions {
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:brewed_potion",
            serde_json::json!({"potion":potion}),
        );
    }
    if let Some((item, levels)) = effects.enchanted_item {
        let item = crate::advancements::item_snapshot(state, &item);
        crate::advancements::trigger(
            state,
            entity,
            "minecraft:enchanted_item",
            serde_json::json!({"item":item,"levels":levels}),
        );
    }
    crate::effects::sync_appearance(state, entity);
    if effects.experience_orbs > 0 {
        let pos = state
            .open_containers
            .get(&entity)
            .map(|&(x, y, z)| [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5])
            .unwrap_or_else(|| state.world.world_spawn_position());
        crate::experience_orbs::award(
            state,
            pos,
            effects.experience_orbs,
            &mut changes.entities_spawned,
        );
    }
    if effects.anvil_used {
        stations::damage_anvil(state, entity, changes);
    }
    for (item, count, kind) in effects.crafted {
        crate::plugin::craft_event(state, entity, item, count, kind);
    }
    if effects.dropped.is_empty() {
        return;
    }
    let pos = state
        .entities
        .slot_of(entity)
        .map(|slot| state.entities.pos(slot))
        .unwrap_or_else(|| state.world.world_spawn_position());
    for stack in effects.dropped {
        let id = spawn_item_entity(state, pos, stack);
        changes.entities_spawned.push(id);
    }
}

/// Whether a placement may overwrite the cell: air, fluids and the vanilla
/// `#minecraft:replaceable` blocks (grass, snow layers, vines, ...); never a
/// torch, sign or flower.
pub(crate) fn replaceable(state: &TickState, current: u32) -> bool {
    if current == state.registry.states.air
        || matches!(
            state.registry.materials.classify(current),
            mc_physics::BlockMaterial::Water | mc_physics::BlockMaterial::Lava
        )
    {
        return true;
    }
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))
        .is_some_and(|block| {
            state.registry.tags.contains_raw_id(
                "minecraft:block",
                "minecraft:replaceable",
                block.block.raw_id,
            )
        })
}

/// Vanilla `updateShape` -> `canSurvive`: around every written cell, an
/// attached block (torch, flower, sign, door, ladder, button) whose support
/// is no longer solid becomes air and drops its loot; a popped door or
/// double plant takes its other half along. Cascades through the pops.
pub(crate) fn pop_unsupported(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let air = state.registry.states.air;
    let mut frontier: Vec<BlockPos> = writes.iter().map(|(pos, _)| *pos).collect();
    while let Some(changed) = frontier.pop() {
        let neighbours = [
            [0, 1, 0],
            [0, -1, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 0, 1],
            [0, 0, -1],
        ]
        .map(|step| BlockPos {
            x: changed.x + step[0],
            y: changed.y + step[1],
            z: changed.z + step[2],
        });
        for pos in neighbours {
            let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
                continue;
            };
            if current == air {
                continue;
            }
            if !crate::placement::support_changed(
                &state.registry.blocks,
                &state.registry.tags,
                pos,
                current,
                changed,
            ) || crate::placement::supported(
                &state.registry.blocks,
                &state.registry.tags,
                pos,
                current,
                &|pos| state.world.block_at(pos.x, pos.y, pos.z),
            ) {
                continue;
            }
            let mut gone = vec![pos];
            if let Some((other, _)) =
                crate::placement::other_half(&state.registry.blocks, pos, current)
                && let Some(other_state) = state.world.block_at(other.x, other.y, other.z)
                && crate::placement::paired(
                    &state.registry.blocks,
                    pos,
                    current,
                    other,
                    other_state,
                )
            {
                gone.push(other);
            }
            if !state.pop_block_loot(pos, current, spawned) {
                continue;
            }
            for cell in gone {
                let old = state
                    .world
                    .block_at(cell.x, cell.y, cell.z)
                    .unwrap_or(current);
                let new = crate::placement::break_state(
                    &state.registry.blocks,
                    old,
                    air,
                    state.registry.states.water,
                );
                state.world.set_block(cell.x, cell.y, cell.z, new);
                writes.retain(|(written, _)| *written != cell);
                writes.push((cell, new));
                frontier.push(cell);
            }
        }
    }
}

/// A `UseItem` with a bucket in `hand`: the fill/empty intent, if the
/// look ray finds a source to take or a cell to pour into.
fn bucket_intent(
    state: &mut TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: crate::boundary::Hand,
    yaw: f32,
    pitch: f32,
) -> Option<Intent> {
    let mut intent = plan_bucket_intent(state, entity, slot, hand, yaw, pitch)?;
    intent.seq = state.next_seq(entity);
    Some(intent)
}

pub(crate) fn plan_bucket_intent(
    state: &TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: crate::boundary::Hand,
    yaw: f32,
    pitch: f32,
) -> Option<Intent> {
    plan_bucket_intent_at(state, entity, slot, hand, yaw, pitch, None)
}

pub(crate) fn bucket_eye(state: &TickState, slot: EntitySlot) -> [f64; 3] {
    let pos = state.entities.pos(slot);
    [
        pos[0],
        pos[1] + player_eye_height(state.entities.row(slot).appearance.pose),
        pos[2],
    ]
}

fn plan_bucket_intent_at(
    state: &TickState,
    entity: EntityId,
    slot: EntitySlot,
    hand: Hand,
    yaw: f32,
    pitch: f32,
    eye: Option<[f64; 3]>,
) -> Option<Intent> {
    use crate::fluids::Fluid;
    let item = |name: &str| {
        mc_data::Identifier::parse(name)
            .ok()
            .and_then(|id| state.registry.items.id_of(&id))
    };
    let (empty, water, lava) = (
        item("minecraft:bucket")?,
        item("minecraft:water_bucket")?,
        item("minecraft:lava_bucket")?,
    );
    let player = state.players.get(&entity)?;
    if player.spectator() {
        return None;
    }
    let held_slot = held_slot_index(player, hand);
    let held = player.inventory.slots.get(held_slot)?;
    let carried = match held.item_id {
        id if held.is_empty() || ![empty, water, lava].contains(&id) => return None,
        id if id == water => Some(Fluid::Water),
        id if id == lava => Some(Fluid::Lava),
        _ => None,
    };
    let eye = eye.unwrap_or_else(|| bucket_eye(state, slot));
    let (target, new) = crate::fluids::bucket_target(state, eye, yaw, pitch, carried)?;
    let current = state.world.block_at(target.x, target.y, target.z)?;
    let result = match carried {
        Some(_) => empty,
        None => match crate::fluids::fluid_at(&state.registry.states, current) {
            Some((Fluid::Lava, _)) => lava,
            _ => water,
        },
    };
    let creative = player.creative();
    let single = held.count == 1;
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::Bucket {
            pos: target,
            give: (!creative && !single).then_some(result),
        },
        footprint: Footprint {
            block_expect: vec![(target, current)],
            block_write: if carried == Some(Fluid::Water) && state.dimension.water_evaporates() {
                Vec::new()
            } else {
                vec![(target, new)]
            },
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(held_slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: i32::from(!creative),
                put_item: (!creative && single).then_some(result),
                put_count: 1,
            }],
            ..Default::default()
        },
    })
}

fn plan_break(state: &TickState, entity: EntityId, position: BlockPos, current: u32) -> Intent {
    let air = state.registry.states.air;
    let mut block_expect = vec![(position, current)];
    let mut block_write = vec![(
        position,
        crate::placement::break_state(
            &state.registry.blocks,
            current,
            air,
            state.registry.states.water,
        ),
    )];
    let mut broken = position;
    if let Some((other, primary)) =
        crate::placement::other_half(&state.registry.blocks, position, current)
        && let Some(other_state) = state.world.block_at(other.x, other.y, other.z)
        && crate::placement::paired(
            &state.registry.blocks,
            position,
            current,
            other,
            other_state,
        )
    {
        block_expect.push((other, other_state));
        block_write.push((
            other,
            crate::placement::break_state(
                &state.registry.blocks,
                other_state,
                air,
                state.registry.states.water,
            ),
        ));
        broken = primary;
    }
    if let Some((other, other_state, single)) = crate::chests::break_partner(
        &state.registry.blocks,
        position,
        current,
        &|pos: BlockPos| state.world.block_at(pos.x, pos.y, pos.z),
    ) {
        block_expect.push((other, other_state));
        block_write.push((other, single));
    }
    Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::BreakBlock { pos: broken },
        footprint: Footprint {
            block_expect,
            block_write,
            ..Default::default()
        },
    }
}

fn plan_placement(
    state: &TickState,
    entity: EntityId,
    hand: crate::boundary::Hand,
    position: BlockPos,
    face: u8,
    cursor: [f32; 3],
    held: &mc_data::ItemStack,
) -> Option<Intent> {
    let click = placement_click(state, entity, position, face, cursor)?;
    plan_placement_click(state, entity, hand, held, click)
}

pub(crate) fn placement_click(
    state: &TickState,
    entity: EntityId,
    position: BlockPos,
    face: u8,
    cursor: [f32; 3],
) -> Option<crate::placement::Click> {
    let slot = state.entities.slot_of(entity)?;
    let [yaw, pitch, _] = state.entities.rot(slot);
    Some(crate::placement::Click {
        clicked: position,
        face: crate::placement::Face::from_code(face),
        cursor,
        yaw,
        pitch,
        sneaking: state.entities.row(slot).appearance.shared_flags & SNEAKING_FLAG != 0,
    })
}

fn plan_placement_click(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    held: &mc_data::ItemStack,
    click: crate::placement::Click,
) -> Option<Intent> {
    let item_block = block_for_item(state, held)?;
    let dependencies = std::cell::RefCell::new(Vec::new());
    let block_at = |pos: BlockPos| {
        let current = state.world.block_at(pos.x, pos.y, pos.z)?;
        let mut dependencies = dependencies.borrow_mut();
        if !dependencies.iter().any(|(at, _)| *at == pos) {
            dependencies.push((pos, current));
        }
        Some(current)
    };
    let can_replace = |current: u32| replaceable(state, current);
    let reads = crate::placement::Reads {
        block_at: &block_at,
        replaceable: &can_replace,
        water_source: state.registry.states.water,
    };
    let cells = crate::placement::place(&state.registry.blocks, item_block, &click, &reads)?;
    if !crate::farming::can_stand(state, cells[0].1, cells[0].0)
        || !crate::placement::supported(
            &state.registry.blocks,
            &state.registry.tags,
            cells[0].0,
            cells[0].1,
            &block_at,
        )
        || cells.iter().any(|(pos, placed)| {
            state.registry.materials.classify(*placed) == mc_physics::BlockMaterial::Solid
                && obstructed(state, *pos)
        })
    {
        return None;
    }
    for (pos, _) in &cells {
        block_at(*pos)?;
    }
    let block_expect = dependencies.into_inner();
    let (target, place_state) = cells[0];
    let player = state.players.get(&entity)?;
    let slot = held_slot_index(player, hand);
    Some(Intent {
        actor: entity,
        seq: 0,
        kind: IntentKind::PlaceBlock {
            pos: target,
            state: place_state,
        },
        footprint: Footprint {
            block_expect,
            block_write: cells,
            inv: vec![InvClaim {
                owner: entity,
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
                expect_item: held.item_id,
                expect_count: held.count,
                take: i32::from(!player.creative()),
                put_item: None,
                put_count: 0,
            }],
            ..Default::default()
        },
    })
}

/// Native early refusals precede the hook, just as they precede native use.
/// The hook-enabled gateway returns a per-viewer correction rather than
/// pretending that a prediction repair is a world/WAL mutation.
pub(crate) fn client_build_refused(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
) -> bool {
    match input {
        PlayerInput::Dig {
            position, action, ..
        } if *action != DigAction::Cancel => {
            !within_block_reach(state, entity, *position)
                || !crate::plugin_player::allows_block(state, entity, *position)
                || crate::plugin_inventories::has_reservations_for_block(state, *position)
                || (*action == DigAction::Finish
                    && state
                        .world
                        .block_at(position.x, position.y, position.z)
                        .is_none_or(|current| {
                            !mining::allows(state, entity, *position, current, *action)
                        }))
        }
        PlayerInput::UseItemOn { position, face, .. } => {
            // A zone's public-use flag lets a non-member flip a door-like block;
            // the click then never places, so only that block is checked.
            if crate::plugin_player::public_use_override(state, entity, *position) {
                return !within_block_reach(state, entity, *position);
            }
            let step = crate::placement::Face::from_code(*face).offset();
            let adjacent = BlockPos {
                x: position.x.saturating_add(step[0]),
                y: position.y.saturating_add(step[1]),
                z: position.z.saturating_add(step[2]),
            };
            !within_block_reach(state, entity, *position)
                || !crate::plugin_player::allows_block(state, entity, *position)
                || !crate::plugin_player::allows_block(state, entity, adjacent)
        }
        _ => false,
    }
}

fn use_intercepts_build(
    state: &TickState,
    entity: EntityId,
    hand: Hand,
    pos: BlockPos,
    face: u8,
    held: &mc_data::ItemStack,
) -> bool {
    if crate::leash::would_use_fence(state, entity, pos)
        || crate::shelf::intercepts_placement(state, hand, pos, face)
        || crate::campfire::intercepts_placement(state, pos, held)
    {
        return true;
    }
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return false;
    };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return false;
    };
    let path = block.block.id.path();
    matches!(path, "jukebox" | "vault" | "respawn_anchor")
        || (path.ends_with("_bed")
            && (!state.dimension.beds_work()
                || crate::placement::other_half(&state.registry.blocks, pos, current).is_some()))
        || crate::copper::family(path).is_some_and(|(family, _, _)| family == "copper_golem_statue")
}

pub(crate) fn plan_client_build(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
) -> Option<Intent> {
    plan_client_build_at(state, entity, input, state.tick, None)
        .filter(|intent| !intent.footprint.block_write.is_empty())
}

pub(crate) fn plan_client_build_at(
    state: &TickState,
    entity: EntityId,
    input: &PlayerInput,
    planning_tick: u64,
    pose: Option<crate::build_precommit::PlanningPose>,
) -> Option<Intent> {
    match input {
        PlayerInput::Dig {
            position, action, ..
        } => {
            let current = state.world.block_at(position.x, position.y, position.z)?;
            if current == state.registry.states.air || !within_block_reach(state, entity, *position)
            {
                return None;
            }
            if !mining::allows(state, entity, *position, current, *action) {
                return None;
            }
            Some(plan_break(state, entity, *position, current))
        }
        PlayerInput::UseItemOn {
            hand,
            position,
            face,
            cursor,
            sequence,
            ..
        } => {
            if !within_block_reach(state, entity, *position) {
                return None;
            }
            let player = state.players.get(&entity)?;
            let held = player.inventory.slots.get(held_slot_index(player, *hand))?;
            if use_intercepts_build(state, entity, *hand, *position, *face, held) {
                return None;
            }
            match crate::composter::plan_use(state, entity, *hand, *position, *sequence) {
                crate::composter::UsePlan::Fill {
                    current,
                    next,
                    maturing,
                } => {
                    return Some(Intent {
                        actor: entity,
                        seq: 0,
                        kind: IntentKind::ComposterFill {
                            pos: *position,
                            maturing,
                        },
                        footprint: Footprint {
                            block_expect: vec![(*position, current)],
                            block_write: vec![(*position, next)],
                            inv: vec![InvClaim {
                                owner: entity,
                                slot: u16::try_from(held_slot_index(player, *hand))
                                    .unwrap_or(u16::MAX),
                                expect_item: held.item_id,
                                expect_count: held.count,
                                take: i32::from(!player.creative()),
                                put_item: None,
                                put_count: 0,
                            }],
                            ..Default::default()
                        },
                    });
                }
                crate::composter::UsePlan::Direct | crate::composter::UsePlan::Collect { .. } => {
                    return None;
                }
                crate::composter::UsePlan::Pass => {}
            }
            if container_menu_for(state, position.x, position.y, position.z).is_some() {
                return None;
            }
            let slot = state.entities.slot_of(entity)?;
            let [yaw, _, _] = state.entities.rot(slot);
            if crate::placement::use_block(
                &state.registry.blocks,
                *position,
                yaw,
                &|pos: BlockPos| state.world.block_at(pos.x, pos.y, pos.z),
            )
            .is_some()
            {
                return None;
            }
            if held.is_empty() {
                return None;
            }
            let item = state.registry.items.name_of(held.item_id)?;
            if item.as_str() == "minecraft:bone_meal" {
                return crate::farming::plan_use_intent_at(
                    state,
                    entity,
                    *hand,
                    *position,
                    crate::placement::Face::from_code(*face),
                    held,
                    planning_tick,
                );
            }
            if matches!(
                item.as_str(),
                "minecraft:bucket" | "minecraft:water_bucket" | "minecraft:lava_bucket"
            ) {
                return crate::fluids::plan_use_on(
                    state,
                    entity,
                    *hand,
                    *position,
                    crate::placement::Face::from_code(*face),
                    held,
                );
            }
            let click = match pose {
                Some(crate::build_precommit::PlanningPose::Placement(click)) => Some(click),
                _ => None,
            };
            if crate::signs::intercepts_placement(
                state,
                entity,
                *hand,
                *position,
                click.map(|click| click.sneaking),
            ) {
                return None;
            }
            match click {
                Some(click) => plan_placement_click(state, entity, *hand, held, click),
                None => plan_placement(state, entity, *hand, *position, *face, *cursor, held),
            }
        }
        PlayerInput::UseItem {
            hand, yaw, pitch, ..
        } => {
            let eye = match pose {
                Some(crate::build_precommit::PlanningPose::Ray { eye }) => Some(eye),
                _ => None,
            };
            plan_bucket_intent_at(
                state,
                entity,
                state.entities.slot_of(entity)?,
                *hand,
                *yaw,
                *pitch,
                eye,
            )
        }
        _ => None,
    }
}

/// Held slot index for a hand (mirrors `PlayerState::held_slot`).
pub(crate) fn held_slot_index(
    player: &crate::player::PlayerState,
    hand: crate::boundary::Hand,
) -> usize {
    match hand {
        crate::boundary::Hand::Main => usize::from(player.inventory.selected.min(8)),
        crate::boundary::Hand::Off => crate::inventory::OFF_HAND,
    }
}

/// Admit an actual native held-item consumption, not the enclosing input.
pub(crate) fn held_debit_below_floor(
    state: &TickState,
    entity: EntityId,
    hand: crate::boundary::Hand,
    count: u32,
) -> bool {
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    if player.creative() {
        return false;
    }
    let slot = held_slot_index(player, hand);
    let held = &player.inventory.slots[slot];
    let debit = count.min(held.count.max(0) as u32);
    crate::plugin_inventories::below_player_debit_floors(state, entity, &[(slot, debit)], None)
}

/// The block the held stack places. Items with no block form (food, tools)
/// return `None`.
fn block_for_item<'a>(
    state: &'a TickState,
    held: &mc_data::ItemStack,
) -> Option<&'a mc_world::Block> {
    let name = state.registry.items.name_of(held.item_id)?;
    crate::farming::placed_block(&state.registry.blocks, name)
}

/// Whether a living body (player or mob) overlaps the cell: vanilla
/// `isUnobstructed` refuses a solid block there.
pub(crate) fn obstructed(state: &TickState, pos: BlockPos) -> bool {
    let (x, y, z) = (f64::from(pos.x), f64::from(pos.y), f64::from(pos.z));
    let intersects = |slot| {
        let body = state.entities.body(slot);
        if !body.alive || matches!(body.kind, EntityKind::Item { .. }) {
            return false;
        }
        let at = state.entities.pos(slot);
        at[0] + body.half_width > x
            && at[0] - body.half_width < x + 1.0
            && at[1] + body.height > y
            && at[1] < y + 1.0
            && at[2] + body.half_width > z
            && at[2] - body.half_width < z + 1.0
    };
    // The live mutation-owned chunk indices include bodies moved or spawned by
    // earlier inputs in this batch; a published view or spawn census does not.
    let extent = state.entities.max_half_width();
    let min_x = mc_physics::mth::floor_i32(x - extent).div_euclid(16);
    let max_x = mc_physics::mth::floor_i32(x + 1.0 + extent).div_euclid(16);
    let min_z = mc_physics::mth::floor_i32(z - extent).div_euclid(16);
    let max_z = mc_physics::mth::floor_i32(z + 1.0 + extent).div_euclid(16);
    let cells =
        (i64::from(max_x) - i64::from(min_x) + 1) * (i64::from(max_z) - i64::from(min_z) + 1);
    // Tiny populations or a previously enormous body can make buckets dearer.
    if cells * 2 >= state.entities.len() as i64 {
        return state.entities.slots().any(intersects);
    }
    for cx in min_x..=max_x {
        for cz in min_z..=max_z {
            let chunk = ChunkPos { x: cx, z: cz };
            for index in [
                state.entities.brain_sensor_chunks(),
                state.entities.villager_chunks(),
            ] {
                if index.get(&chunk).is_some_and(|ids| {
                    ids.iter()
                        .filter_map(|id| state.entities.slot_of_any(*id))
                        .any(intersects)
                }) {
                    return true;
                }
            }
        }
    }
    false
}

/// Container menu for the block at `(x, y, z)`: (kind, title, slots). Both
/// halves of a double chest open one 54-slot menu.
fn container_menu_for(
    state: &TickState,
    x: i32,
    y: i32,
    z: i32,
) -> Option<(crate::inventory::MenuKind, String, usize)> {
    let current = state.world.block_at(x, y, z)?;
    let menu = container_kind_for_state(state, current)?;
    if container_cells(state, (x, y, z)).len() == 2 {
        return Some((
            crate::inventory::MenuKind::Chest { rows: 6 },
            "Large Chest".to_owned(),
            54,
        ));
    }
    Some(menu)
}

/// Storage cells behind the container at `pos`, in menu order: one, or the
/// two halves of a double chest.
fn container_cells(state: &TickState, pos: (i32, i32, i32)) -> Vec<(i32, i32, i32)> {
    let at = BlockPos {
        x: pos.0,
        y: pos.1,
        z: pos.2,
    };
    crate::chests::storage_cells(&state.registry.blocks, at, &|cell: BlockPos| {
        state.world.block_at(cell.x, cell.y, cell.z)
    })
    .into_iter()
    .map(|cell| (cell.x, cell.y, cell.z))
    .collect()
}

/// The contents of `cells` back to back (menu order).
fn container_slots(state: &TickState, cells: &[(i32, i32, i32)]) -> Vec<mc_data::ItemStack> {
    cells
        .iter()
        .filter_map(|cell| state.block_entities.get(cell))
        .flat_map(|record| record.slots.iter().cloned())
        .collect()
}

/// Resync the real current menu after refusing a native inventory transition.
/// A vehicle or block menu must never be imaged with placeholder empty cargo.
pub(crate) fn inventory_transition_refused(
    state: &TickState,
    entity: EntityId,
) -> crate::player::PlayerEffects {
    let Some(player) = state.players.get(&entity) else {
        return crate::player::PlayerEffects::default();
    };
    if let Some(slots) = crate::vehicles::container_slots_for_player(state, entity) {
        return player.inventory_transition_refused(slots);
    }
    let cells = state
        .open_containers
        .get(&entity)
        .map(|open| container_cells(state, *open))
        .unwrap_or_default();
    player.inventory_transition_refused(&container_slots(state, &cells))
}

/// Publish the current authoritative menu image to every viewer after external
/// native custody mutation, with one fresh revision per affected menu.
pub(crate) fn refresh_container_viewers(state: &mut TickState, position: BlockPos) {
    let cells = container_cells(state, (position.x, position.y, position.z));
    let viewers: Vec<_> = state
        .open_containers
        .iter()
        .filter(|(_, open)| container_cells(state, **open) == cells)
        .filter_map(|(viewer, _)| {
            state
                .connections
                .entity_connection(*viewer)
                .map(|connection| (*viewer, connection))
        })
        .collect();
    if viewers.is_empty() {
        return;
    }
    let contents = container_slots(state, &cells);
    for (viewer, connection) in viewers {
        let Some(player) = state.players.get_mut(&viewer) else {
            continue;
        };
        if let Some(menu) = player.open.as_mut() {
            menu.state_id = (menu.state_id + 1) & 0x7fff;
        }
        for change in player.sync_container_view(&contents) {
            state.push_player(connection, change);
        }
    }
}

/// Write menu-order `slots` back over `cells`; a half whose contents
/// changed is marked dirty.
fn store_container_slots(
    state: &mut TickState,
    cells: &[(i32, i32, i32)],
    slots: &[mc_data::ItemStack],
) {
    let mut rest = slots;
    for cell in cells {
        let Some(record) = state.block_entities.get_mut(cell) else {
            continue;
        };
        let (mine, tail) = rest.split_at(record.slots.len().min(rest.len()));
        rest = tail;
        if record.slots != mine {
            record.slots.clear();
            record.slots.extend_from_slice(mine);
            crate::brewing::slots_replaced(&mut state.brewing, *cell, &record.slots);
            let position = BlockPos {
                x: cell.0,
                y: cell.1,
                z: cell.2,
            };
            crate::redstone::containers::inventory_changed(
                &mut state.redstone_containers,
                position,
            );
            state.world.mark_block_entity_dirty(position);
        }
    }
}

/// Container menu for a block state id.
fn container_kind_for_state(
    state: &TickState,
    state_id: u32,
) -> Option<(crate::inventory::MenuKind, String, usize)> {
    let block_state = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(state_id))?;
    let id = block_state.block.id.as_str();
    if let Some(menu) = stations::craft_menu_for_block(id) {
        return Some(menu);
    }
    if crate::copper::chest(block_state.block.id.path()) {
        return Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Copper Chest".into(),
            27,
        ));
    }
    if crate::shulker::is_block(block_state.block.id.path()) {
        return Some((
            crate::inventory::MenuKind::ShulkerBox,
            "Shulker Box".into(),
            27,
        ));
    }
    match id {
        "minecraft:chest" | "minecraft:trapped_chest" => Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Chest".to_owned(),
            27,
        )),
        "minecraft:hopper" => Some((crate::inventory::MenuKind::Hopper, "Hopper".into(), 5)),
        "minecraft:crafter" => Some((crate::inventory::MenuKind::Crafter, "Crafter".into(), 9)),
        "minecraft:brewing_stand" => Some((
            crate::inventory::MenuKind::BrewingStand,
            "Brewing Stand".into(),
            5,
        )),
        "minecraft:dispenser" | "minecraft:dropper" => Some((
            crate::inventory::MenuKind::Dispenser,
            if id == "minecraft:dropper" {
                "Dropper"
            } else {
                "Dispenser"
            }
            .into(),
            9,
        )),
        "minecraft:barrel" => Some((
            crate::inventory::MenuKind::Chest { rows: 3 },
            "Barrel".to_owned(),
            27,
        )),
        "minecraft:crafting_table" => Some((
            crate::inventory::MenuKind::CraftingTable,
            "Crafting Table".to_owned(),
            10,
        )),
        "minecraft:beacon" => Some((crate::inventory::MenuKind::Beacon, "Beacon".into(), 0)),
        "minecraft:enchanting_table" => Some((
            crate::inventory::MenuKind::EnchantingTable,
            "Enchant".into(),
            0,
        )),
        "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => {
            Some((crate::inventory::MenuKind::Anvil, "Repair & Name".into(), 0))
        }
        "minecraft:grindstone" => Some((
            crate::inventory::MenuKind::Grindstone,
            "Repair & Disenchant".into(),
            0,
        )),
        "minecraft:furnace" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::Furnace),
            "Furnace".to_owned(),
            3,
        )),
        "minecraft:smoker" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::Smoker),
            "Smoker".to_owned(),
            3,
        )),
        "minecraft:blast_furnace" => Some((
            crate::inventory::MenuKind::Furnace(mc_data::recipes::CookingKind::BlastFurnace),
            "Blast Furnace".to_owned(),
            3,
        )),
        _ => None,
    }
}

/// Tick every furnace block entity: cooking advances, slots mutate in
/// place. `lit` block property follows the burn state when resolvable.
pub(crate) fn tick_furnaces(
    state: &mut TickState,
    block_writes: &mut Vec<(BlockPos, u32)>,
    catch_up: Option<(ChunkPos, u64)>,
) {
    // Independent borrow of the registry (Arc): the storage pass below needs
    // `&mut state`.
    let registry = std::sync::Arc::clone(&state.registry);
    let rules = registry.item_rules();
    // Collect positions first: the tick mutates storage per entity.
    let positions: Vec<(i32, i32, i32)> = state.block_entities.keys().copied().collect();
    for pos in positions {
        let chunk = ChunkPos {
            x: pos.0.div_euclid(16),
            z: pos.2.div_euclid(16),
        };
        if catch_up.is_some_and(|(target, _)| target != chunk)
            || (catch_up.is_none() && state.world.simulation_debt(chunk) > 0)
        {
            continue;
        }
        if !state
            .world
            .is_active(pos.0.div_euclid(16), pos.2.div_euclid(16))
        {
            continue;
        }
        let Some(current) = state.world.block_at(pos.0, pos.1, pos.2) else {
            continue;
        };
        let kind = match furnace_kind_for_state(&registry, current) {
            Some(kind) => kind,
            None => continue,
        };
        if catch_up.is_some() {
            let expect = Footprint {
                block_expect: vec![(
                    BlockPos {
                        x: pos.0,
                        y: pos.1,
                        z: pos.2,
                    },
                    current,
                )],
                ..Default::default()
            };
            if !state.footprint_valid(&expect) {
                continue;
            }
        }
        let position = BlockPos {
            x: pos.0,
            y: pos.1,
            z: pos.2,
        };
        let candidate = if crate::plugin_inventories::has_reservations_for_block(state, position) {
            let Some(record) = state.block_entities.get(&pos) else {
                continue;
            };
            let mut slots = record.slots.clone();
            if slots.len() < 3 {
                slots.resize(3, mc_data::ItemStack::EMPTY);
            }
            let mut progress = record.furnace.clone().unwrap_or_default();
            let before = progress.properties();
            let outcome = match catch_up {
                Some((_, elapsed)) => progress.catch_up(&rules, kind, &mut slots, elapsed),
                None => progress.tick(&rules, kind, &mut slots),
            };
            let after = progress.properties();
            if crate::plugin_inventories::below_block_floors(state, position, &slots) {
                continue;
            }
            Some((slots, progress, before, outcome, after))
        } else {
            None
        };
        let record = if catch_up.is_some() {
            state.block_entities.get_mut_catch_up(&pos)
        } else {
            state.block_entities.get_mut(&pos)
        };
        let Some(record) = record else {
            continue;
        };
        let (before, outcome, after) = if let Some((slots, progress, before, outcome, after)) =
            candidate
        {
            record.slots = slots;
            record.furnace = Some(progress);
            (before, outcome, after)
        } else {
            if record.slots.len() < 3 {
                record.slots.resize(3, mc_data::ItemStack::EMPTY);
            }
            let progress = record.furnace.get_or_insert_with(Default::default);
            let before = progress.properties();
            let outcome = match catch_up {
                Some((_, elapsed)) => progress.catch_up(&rules, kind, &mut record.slots, elapsed),
                None => progress.tick(&rules, kind, &mut record.slots),
            };
            (before, outcome, progress.properties())
        };
        if !outcome.changed && before == after {
            continue;
        }
        for (viewer, open) in &state.open_containers {
            if *open != pos {
                continue;
            }
            let Some(player) = state.players.get_mut(viewer) else {
                continue;
            };
            let Some(menu) = player.open.as_mut() else {
                continue;
            };
            let connection = player.connection;
            let window = menu.window;
            let queue = state.outbox.entry(connection).or_default();
            if outcome.changed {
                menu.state_id = (menu.state_id + 1) & 0x7FFF;
                queue.extend(player.sync_container_view(&record.slots));
            }
            for (property, (old, value)) in before.into_iter().zip(after).enumerate() {
                if old != value {
                    queue.push(PlayerChange::MenuProperty {
                        window,
                        property: property as i16,
                        value: value as i16,
                    });
                }
            }
        }
        if outcome.changed {
            crate::redstone::containers::inventory_changed(
                &mut state.redstone_containers,
                position,
            );
        }
        if let Some(lit) = outcome.lit {
            set_lit_prop(state, pos, lit, block_writes, catch_up.is_some());
        }
    }
}

/// Cooking kind for a furnace-family block state.
fn furnace_kind_for_state(
    registry: &CoreRegistry,
    state_id: u32,
) -> Option<mc_data::recipes::CookingKind> {
    let block_state = registry.blocks.by_id(mc_world::BlockStateId(state_id))?;
    match block_state.block.id.as_str() {
        "minecraft:furnace" => Some(mc_data::recipes::CookingKind::Furnace),
        "minecraft:smoker" => Some(mc_data::recipes::CookingKind::Smoker),
        "minecraft:blast_furnace" => Some(mc_data::recipes::CookingKind::BlastFurnace),
        _ => None,
    }
}

/// Set the `lit` property on the block at `pos` (no-op when unresolvable).
fn set_lit_prop(
    state: &mut TickState,
    pos: (i32, i32, i32),
    lit: bool,
    block_writes: &mut Vec<(BlockPos, u32)>,
    catch_up: bool,
) {
    let Some(current) = state.world.block_at(pos.0, pos.1, pos.2) else {
        return;
    };
    let Some(block_state) = state.registry.blocks.by_id(mc_world::BlockStateId(current)) else {
        return;
    };
    let mut properties = block_state.properties.clone();
    let Some((_, value)) = properties.iter_mut().find(|(key, _)| key == "lit") else {
        return;
    };
    *value = if lit { "true" } else { "false" }.to_owned();
    if let Some(next) = state
        .registry
        .blocks
        .by_name_and_props(&block_state.block.id, &properties)
    {
        if catch_up {
            state.world.set_block_catch_up(pos.0, pos.1, pos.2, next.0);
        } else {
            state.world.set_block(pos.0, pos.1, pos.2, next.0);
        }
        block_writes.push((
            BlockPos {
                x: pos.0,
                y: pos.1,
                z: pos.2,
            },
            next.0,
        ));
    }
}

/// Pull resident chunks' typed block-entity maps into live storage.
/// Journal post-images overwrite per position; unloaded chunks' records stay
/// until their chunk reloads (eviction does not delete storage).
fn sync_chunk_block_entities(state: &mut TickState) {
    for chunk_pos in state.world.take_hydration_chunks() {
        let Some(chunk) = state.world.chunk(chunk_pos) else {
            continue;
        };
        let trial_admission =
            crate::trial_spawner::hydrate_chunk(&mut state.trial_spawners, &state.registry, chunk)
                .and_then(|()| {
                    crate::vault::hydrate_chunk(&mut state.vaults, &state.registry, chunk)
                });
        if let Err(error) = trial_admission {
            state.trial_spawners.retain(|&(x, _, z), _| {
                x.div_euclid(16) != chunk_pos.x || z.div_euclid(16) != chunk_pos.z
            });
            state.vaults.retain(|&(x, _, z), _| {
                x.div_euclid(16) != chunk_pos.x || z.div_euclid(16) != chunk_pos.z
            });
            state.world.reject_resident(chunk_pos, error);
            continue;
        }
        crate::campfire::hydrate_chunk(&mut state.campfires, &state.registry, chunk);
        crate::crafter::hydrate_chunk(
            &mut state.crafters,
            &mut state.block_entities,
            &state.registry,
            chunk,
        );
        crate::shelf::hydrate_chunk(&mut state.block_entities, &state.registry, chunk);
        if let Err(error) =
            crate::shulker::hydrate_chunk(&mut state.block_entities, &state.registry, chunk)
        {
            state.block_entities.retain(|&(x, _, z), _| {
                x.div_euclid(16) != chunk_pos.x || z.div_euclid(16) != chunk_pos.z
            });
            state.world.reject_resident(chunk_pos, error.to_string());
            continue;
        }
        if let Err(error) = crate::bees::hydrate_chunk(&mut state.hives, &state.registry, chunk) {
            state.hives.retain(|&(x, _, z), _| {
                x.div_euclid(16) != chunk_pos.x || z.div_euclid(16) != chunk_pos.z
            });
            state.world.reject_resident(chunk_pos, error.to_string());
            continue;
        }
        crate::jukebox::hydrate_chunk(
            &mut state.jukeboxes,
            &mut state.block_entities,
            &state.registry,
            chunk,
        )
        .unwrap_or_else(|error| panic!("resident jukebox rejected: {error}"));
        crate::brewing::hydrate_chunk(
            &mut state.brewing,
            &mut state.block_entities,
            &state.registry,
            chunk,
        );
        state.block_ticks.hydrate_chunk(chunk, state.tick);
        crate::redstone::containers::hydrate_chunk(
            &mut state.block_entities,
            &mut state.redstone_containers,
            &state.registry,
            chunk,
            state.tick,
        );
        for (block, chest) in &chunk.chests {
            state
                .block_entities
                .hydrate((block.x, block.y, block.z), || BlockEntityState {
                    slots: chest.slots.iter().map(furnace_slot_to_stack).collect(),
                    furnace: None,
                });
        }
        for (block, furnace) in &chunk.furnaces {
            // Hydrate ONLY on a vacant live entry: the live record is
            // authoritative (player mutations, replay images). Overwriting
            // it from the chunk every tick restores extracted items and
            // loses inserted ones.
            if state
                .block_entities
                .contains_key(&(block.x, block.y, block.z))
            {
                continue;
            }
            let mut slots = vec![mc_data::ItemStack::EMPTY; 3];
            for (index, slot) in furnace.slots.iter().enumerate() {
                slots[index] = furnace_slot_to_stack(slot);
            }
            state
                .block_entities
                .hydrate((block.x, block.y, block.z), || BlockEntityState {
                    slots,
                    furnace: Some(crate::inventory::furnace::FurnaceState {
                        lit_remaining: i32::from(furnace.burn_remaining),
                        lit_total: i32::from(furnace.burn_total),
                        cook_progress: i32::from(furnace.cook_progress),
                        cook_total: i32::from(furnace.cook_total),
                        recipes_used: std::collections::BTreeMap::new(),
                    }),
                });
        }
    }
}

/// Chunk slot shape -> core item stack.
fn furnace_slot_to_stack(slot: &mc_world::FurnaceSlot) -> mc_data::ItemStack {
    mc_data::ItemStack {
        count: slot.count,
        item_id: slot.item_id,
        damage: slot.damage,
        enchantments: slot.enchantments.clone(),
        custom_name: slot
            .custom_name
            .as_deref()
            .map(str::to_owned)
            .map(String::into_boxed_str),
        item_model: slot.item_model.clone(),
        stew_effects: slot.stew_effects.clone(),
        components: slot.components.clone(),
    }
}

/// Spawn one item entity immediately (input-step effect, not merge: the
/// player already paid the items in `PlayerState`). Returns its entity id.
/// Journaled as an Entity post-image + entities_spawned entry.
pub(crate) fn spawn_item_entity(
    state: &mut TickState,
    pos: [f64; 3],
    stack: mc_data::ItemStack,
) -> EntityId {
    if !state.registry.tags.contains_raw_id(
        "minecraft:item",
        "minecraft:dampens_vibrations",
        stack.item_id as i32,
    ) {
        crate::vibration::emit(state, pos, crate::vibration::GameEvent::EntityPlace, None);
    }
    let id = state.entities.alloc_id();
    let uuid = state.entity_uuid(id);
    let type_id = state.registry.item_type_id;
    let appearance = std::sync::Arc::new(EntityAppearance {
        item: Some(stack.clone()),
        health: 1.0,
        ..Default::default()
    });
    state.entities.spawn(
        id,
        uuid,
        EntityKind::Item { type_id },
        ColdFields {
            pos,
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            ground: false,
            health: 1.0,
            breed_cooldown: 0,
            pickup_delay: 10,
            item: Some(crate::entities::StackRow {
                item_id: stack.item_id,
                count: stack.count,
                stack: Some(stack),
            }),
            appearance,
            movement: crate::blueprint::MovementClass::Item,
            half_width: 0.125,
            height: 0.25,
            speed: 0.0,
            attack_damage: 0.0,
            sight_range: 0.0,
            attack_reach: 0.0,
        },
    );
    id
}

/// Resolve command targets through the same selector engine as all commands.
pub(crate) fn resolve_target(
    state: &TickState,
    executor: EntityId,
    target: &crate::commands::Target,
) -> Vec<EntityId> {
    crate::command_execution::resolve_target(state, executor, target)
}

/// Keep the key-down flag distinct from crouching forced by a low ceiling.
fn update_player_crouching(
    state: &mut TickState,
    entity: EntityId,
    slot: EntitySlot,
    sneaking: bool,
) {
    // Sleeping, swimming, flying, and other poses keep their own dimensions.
    if state.sleepers.contains_key(&entity) {
        return;
    }
    let row = state.entities.row(slot);
    if !matches!(row.appearance.pose, 0 | CROUCHING_POSE) {
        return;
    }
    let half_width = state.entities.body(slot).half_width;
    let position = state.entities.pos(slot);
    // Expanding a crouching box to standing occupies the same space
    // as sweeping the crouching box upward by the height difference.
    let crouching = sneaking
        || (row.appearance.pose == CROUCHING_POSE
            && !mc_physics::resolve_collision_displacement(
                &TickSampler {
                    world: &state.world,
                    registry: &state.registry,
                },
                mc_physics::Vec3::new(position[0], position[1], position[2]),
                mc_physics::Aabb {
                    half_width,
                    height: 1.5,
                },
                mc_physics::Vec3::new(0.0, 0.3, 0.0),
                false,
                0.0,
            )
            .is_some_and(|resolved| (resolved.delta.y - 0.3).abs() < 1e-7));
    let (body, row) = state.entities.parts_mut(slot);
    body.height = if crouching { 1.5 } else { 1.8 };
    let appearance = Arc::make_mut(&mut row.appearance);
    let flags = if sneaking {
        appearance.shared_flags | SNEAKING_FLAG
    } else {
        appearance.shared_flags & !SNEAKING_FLAG
    };
    let pose = if crouching { CROUCHING_POSE } else { 0 };
    if (flags, pose) != (appearance.shared_flags, appearance.pose) {
        appearance.shared_flags = flags;
        appearance.pose = pose;
        state.appearance_changed.push(entity);
    }
}

/// Eye heights for vanilla player poses, independent of the sneak key.
pub(crate) fn player_eye_height(pose: u8) -> f64 {
    match pose {
        2 => 0.2,         // sleeping
        1 | 3 | 4 => 0.4, // fall flying, swimming, spin attack
        5 => 1.27,        // crouching (including forced crouch)
        _ => 1.62,
    }
}

/// Vanilla's block interaction gate measures eye distance to the block AABB,
/// with the server packet handlers allowing a one-block prediction buffer.
pub(crate) fn within_block_reach(state: &TickState, entity: EntityId, target: BlockPos) -> bool {
    let Some(slot) = state.entities.slot_of(entity) else {
        return false;
    };
    let Some(player) = state.players.get(&entity) else {
        return false;
    };
    let position = state.entities.pos(slot);
    let eye_height = player_eye_height(state.entities.row(slot).appearance.pose);
    let eye = [position[0], position[1] + eye_height, position[2]];
    let minimum = [
        f64::from(target.x),
        f64::from(target.y),
        f64::from(target.z),
    ];
    let distance_squared: f64 = eye
        .into_iter()
        .zip(minimum)
        .map(|(coordinate, minimum)| {
            let delta = coordinate - coordinate.clamp(minimum, minimum + 1.0);
            delta * delta
        })
        .sum();
    let range = if player.creative() { 5.0 } else { 4.5 } + 1.0;
    distance_squared < range * range
}

/// Vanilla entity interaction range (3, or 5 in creative), plus the packet
/// handler's three-block prediction buffer, measured eye-to-target AABB.
fn within_entity_reach(state: &TickState, entity: EntityId, target: EntityId) -> bool {
    let (Some(actor_slot), Some(target_slot), Some(player)) = (
        state.entities.slot_of(entity),
        state
            .entities
            .slot_of(crate::end_fight::attack_target(state, target).0),
        state.players.get(&entity),
    ) else {
        return false;
    };
    let position = state.entities.pos(actor_slot);
    let eye = [
        position[0],
        position[1] + player_eye_height(state.entities.row(actor_slot).appearance.pose),
        position[2],
    ];
    let position = state.entities.pos(target_slot);
    let body = state.entities.body(target_slot);
    let (minimum, maximum) = crate::end_fight::part_bounds(state, target).unwrap_or((
        [
            position[0] - body.half_width,
            position[1],
            position[2] - body.half_width,
        ],
        [
            position[0] + body.half_width,
            position[1] + body.height,
            position[2] + body.half_width,
        ],
    ));
    let distance_squared: f64 = eye
        .into_iter()
        .zip(minimum.into_iter().zip(maximum))
        .map(|(coordinate, (minimum, maximum))| {
            let delta = coordinate - coordinate.clamp(minimum, maximum);
            delta * delta
        })
        .sum();
    let range = if player.creative() { 5.0 } else { 3.0 } + 3.0;
    distance_squared < range * range
}

/// Dig/place/attack/breed emit intents; the rest apply directly.
fn apply_inputs<'a>(
    state: &mut TickState,
    inputs: impl IntoIterator<Item = &'a PlayerInput>,
    intents: &mut Vec<Intent>,
    changes: &mut ChangeSet,
) {
    for input in inputs {
        if let PlayerInput::BuildPreviewFence { fence } = input {
            input::interact::build_preview_fence(fence);
            continue;
        }
        if auth::refuse_input(state, input) {
            continue;
        }
        if input.is_communication()
            && !input::social::admit_communication(state, input.connection())
        {
            continue;
        }
        if let PlayerInput::AuthorizedBuild { authorization } = input {
            crate::build_precommit::apply_authorized(state, authorization, intents);
            continue;
        }
        if let PlayerInput::BuildResync { request } = input {
            crate::build_precommit::apply_resync(state, request);
            continue;
        }
        if crate::build_precommit::requires_approval(state, input) {
            crate::build_precommit::resync_input(state, input);
            continue;
        }
        crate::plugin::observe_input(state, input);
        let (handled, event) = crate::plugin_player::menu_input(state, input);
        crate::plugin::enqueue_result(state, event);
        if handled {
            continue;
        }
        if let PlayerInput::PluginClientReady { connection } = input {
            input::session::plugin_client_ready(state, connection);
            continue;
        }
        if let PlayerInput::LoaderViewRequest {
            connection,
            owner,
            request_kind,
        } = input
        {
            crate::plugin_player::loader_view_request(
                state,
                *connection,
                owner,
                request_kind.clone(),
            );
            continue;
        }
        match input {
            PlayerInput::BuildPreviewFence { .. }
            | PlayerInput::AuthorizedBuild { .. }
            | PlayerInput::BuildResync { .. } => {}
            PlayerInput::SeenAdvancements { connection, tab } => {
                input::social::seen_advancements(state, (connection, tab))
            }
            PlayerInput::PluginClientReady { .. } => {}
            PlayerInput::LoaderViewRequest { .. } => {}
            PlayerInput::PermissionLevel { connection, level } => {
                input::session::permission_level(state, (connection, level))
            }
            PlayerInput::SetViewDistance {
                connection,
                view_distance,
            } => input::session::set_view_distance(state, (connection, view_distance)),
            PlayerInput::SetLocale { connection, locale } => {
                input::session::set_locale(state, (connection, locale))
            }
            PlayerInput::Join {
                connection,
                uuid,
                name,
                permission_level,
            } => input::session::join(state, (connection, uuid, name, permission_level), changes),
            PlayerInput::Leave { connection } => input::session::leave(state, connection, changes),
            PlayerInput::RequestStats { connection } => {
                input::social::request_stats(state, connection)
            }
            PlayerInput::ConfirmTeleport {
                connection,
                teleport_id,
            } => input::session::confirm_teleport(state, (connection, teleport_id)),
            PlayerInput::Move { connection, .. }
            | PlayerInput::MoveRotation { connection, .. }
            | PlayerInput::MoveStatus { connection, .. } => {
                input::movement::move_input(state, connection, input, intents)
            }
            PlayerInput::SelectSlot { connection, slot } => {
                input::inventory::select_slot(state, (connection, slot))
            }
            PlayerInput::Dig {
                connection,
                position,
                action,
                sequence,
            } => input::interact::dig(
                state,
                (connection, position, action, sequence),
                intents,
                changes,
            ),
            PlayerInput::UseItemOn {
                connection,
                position,
                hand,
                face,
                cursor,
                sequence,
            } => input::interact::use_item_on(
                state,
                (connection, position, hand, face, cursor, sequence),
                input,
                intents,
                changes,
            ),
            PlayerInput::Attack { connection, target } => {
                input::combat::attack(state, (connection, target), intents, changes)
            }
            PlayerInput::InteractEntity {
                connection,
                target,
                hand,
                location,
            }
            | PlayerInput::InteractEntitySecondary {
                connection,
                target,
                location,
                hand,
            } => input::interact::interact_entity(
                state,
                (connection, target, hand, location),
                input,
                intents,
                changes,
            ),
            PlayerInput::ContainerClick {
                connection,
                window,
                state_id,
                slot,
                button,
                mode,
            } => input::inventory::container_click(
                state,
                (connection, window, state_id, slot, button, mode),
                changes,
            ),
            PlayerInput::SelectTrade { connection, offer } => {
                input::inventory::select_trade(state, (connection, offer), changes)
            }
            PlayerInput::ContainerSlotState {
                connection,
                window,
                slot,
                enabled,
            } => input::inventory::container_slot_state(state, (connection, window, slot, enabled)),
            PlayerInput::ContainerButton {
                connection,
                window,
                button,
            } => input::inventory::container_button(state, (connection, window, button), changes),
            PlayerInput::SetBeacon {
                connection,
                primary,
                secondary,
            } => crate::beacon::configure(state, *connection, *primary, *secondary),
            PlayerInput::RenameItem { connection, name } => {
                input::inventory::rename_item(state, (connection, name), changes)
            }
            PlayerInput::ContainerClose { connection, .. } => {
                input::inventory::container_close(state, connection, changes)
            }
            PlayerInput::PlaceRecipe {
                connection,
                window,
                recipe,
                make_all,
            } => input::inventory::place_recipe(
                state,
                (connection, window, recipe, make_all),
                changes,
            ),
            PlayerInput::SwapHands { connection } => {
                input::inventory::swap_hands(state, connection, changes)
            }
            PlayerInput::CreativeSlot {
                connection,
                slot,
                stack,
            } => input::inventory::creative_slot(state, (connection, slot, stack), changes),
            PlayerInput::Sneak {
                connection,
                sneaking,
            } => input::movement::sneak(state, (connection, sneaking)),
            PlayerInput::VehicleInput { connection, flags } => {
                input::movement::vehicle_input(state, (connection, flags))
            }
            PlayerInput::MoveVehicle {
                connection,
                position,
                yaw,
                pitch,
                on_ground,
            } => {
                input::movement::move_vehicle(state, (connection, position, yaw, pitch, on_ground))
            }
            PlayerInput::Sprint {
                connection,
                sprinting,
            } => input::movement::sprint(state, (connection, sprinting)),
            PlayerInput::StartFallFlying { connection } => elytra::start(state, *connection),
            PlayerInput::SignUpdate {
                connection,
                position,
                front,
                lines,
            } => input::social::sign_update(state, (connection, position, front, lines)),
            PlayerInput::DropSelected {
                connection,
                whole_stack,
            } => input::inventory::drop_selected(state, (connection, whole_stack), changes),
            PlayerInput::UseItem {
                connection,
                hand,
                yaw,
                pitch,
                sequence,
            } => input::interact::use_item(
                state,
                (connection, hand, yaw, pitch, sequence),
                intents,
                changes,
            ),
            PlayerInput::ReleaseUseItem { connection } => {
                input::interact::release_use_item(state, connection)
            }
            PlayerInput::Chat {
                connection,
                message,
            } => input::social::chat(state, (connection, message)),
            PlayerInput::DialogClick {
                connection,
                id,
                payload,
            } => crate::dialog::route_click(state, *connection, id, payload.as_ref(), changes),
            PlayerInput::Command {
                connection,
                command,
                permission_level,
            } => {
                crate::command_console::execute(
                    state,
                    *connection,
                    command,
                    *permission_level,
                    changes,
                );
            }
            PlayerInput::ConsoleCommand(command) => {
                command.execute(state, changes);
            }
            #[cfg(test)]
            PlayerInput::TestGive {
                connection,
                slot,
                stack,
            } => input::inventory::test_give(state, (connection, slot, stack), changes),
            PlayerInput::Respawn { connection } => {
                input::session::respawn(state, connection, changes)
            }
            PlayerInput::LeaveBed { connection } => input::movement::leave_bed(state, connection),
        }
    }
}

/// Shared manual/immediate respawn transition; live rows cannot respawn twice.
pub(crate) fn respawn_player(
    state: &mut TickState,
    connection: ConnectionId,
    changes: &mut ChangeSet,
) -> bool {
    let Some(entity) = state.connections.by_connection.get(&connection).copied() else {
        return true;
    };
    let Some(slot) = state.entities.slot_of_any(entity) else {
        return true;
    };
    if state.entities.body(slot).alive {
        return true;
    }
    crate::mounts::reset_player(state, entity);
    let destination =
        state
            .players
            .get(&entity)
            .map_or(crate::dimensions::Dimension::Overworld, |player| {
                if player.respawn_point.is_some() {
                    player.respawn_dimension
                } else {
                    crate::dimensions::Dimension::Overworld
                }
            });
    if destination != state.dimension {
        state.dimension_respawns.push(entity);
        return true;
    }
    if !crate::sleep::respawn_chunks_loaded(state, entity) {
        return false;
    }
    let personal = crate::sleep::respawn_position(state, entity);
    if personal.is_none() && state.dimension != crate::dimensions::Dimension::Overworld {
        if let Some(player) = state.players.get_mut(&entity) {
            player.respawn_point = None;
            player.respawn_dimension = crate::dimensions::Dimension::Overworld;
        }
        state.dimension_respawns.push(entity);
        return true;
    }
    let [yaw, pitch] = crate::sleep::respawn_rotation(state, entity);
    if personal.is_some()
        && let Some([x, y, z]) = state
            .players
            .get(&entity)
            .and_then(|player| player.respawn_point)
        && let Some(current) = state.world.block_at(x, y, z)
        && let Some(charges) = crate::dimensions::rules::anchor_charge(state, current)
        && charges > 0
        && let Some(next) = crate::dimensions::rules::anchor_state(state, current, charges - 1)
    {
        let position = BlockPos { x, y, z };
        state.world.set_block(x, y, z, next);
        changes.blocks.push(crate::boundary::BlockChange {
            position,
            state: next,
        });
        for cell in crate::plants::neighbours(position) {
            crate::redstone::schedule_cell(state, cell, 1, 0);
        }
    }
    crate::sleep::wake(state, entity);
    let position = personal.unwrap_or_else(|| {
        crate::natural_spawn::player_spawn(state, u64::from(entity.0 as u32) ^ state.tick)
    });
    let (body, row) = state.entities.parts_mut(slot);
    body.alive = true;
    row.health = 20.0;
    row.protected_until = state.tick + crate::entities::SPAWN_INVULNERABLE_TICKS;
    row.embedded_projectile_timers = [0; 2];
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.health = 20.0;
    appearance.arrow_count = 0;
    appearance.stinger_count = 0;
    state.appearance_changed.push(entity);
    state.entities.set_pos(slot, position);
    state.entities.set_rot(slot, yaw, pitch);
    state.entities.set_vel(slot, [0.0; 3]);
    let effects = state
        .players
        .get_mut(&entity)
        .map(|player| player.respawn(position, yaw, pitch, state.dimension))
        .unwrap_or_default();
    apply_player_effects(state, connection, entity, effects, changes);
    crate::bossbar::sync_dimension(state, connection);
    true
}

/// Live player bodies in dense-slot order. A player-kind row without a
/// `players` entry has no tick work, so enumerate the entries instead of
/// reading every resident row; stale entries fail the kind/alive gate.
pub(crate) fn live_players(state: &TickState) -> Vec<(EntitySlot, EntityId, ConnectionId)> {
    let mut found: Vec<(EntitySlot, EntityId, ConnectionId)> = state
        .players
        .keys()
        .filter_map(|id| {
            let slot = state.entities.slot_of(*id)?;
            let body = state.entities.body(slot);
            match body.kind {
                EntityKind::Player { connection } if body.alive => {
                    Some((slot, body.id, connection))
                }
                _ => None,
            }
        })
        .collect();
    found.sort_unstable_by_key(|(slot, ..)| slot.0);
    found
}

/// The previous dense scan, kept as the differential oracle for `live_players`.
#[cfg(test)]
pub(crate) fn live_players_by_scan(state: &TickState) -> Vec<(EntitySlot, EntityId, ConnectionId)> {
    state
        .entities
        .slots()
        .filter_map(|slot| {
            let body = state.entities.body(slot);
            match body.kind {
                EntityKind::Player { connection } if body.alive => {
                    Some((slot, body.id, connection))
                }
                _ => None,
            }
        })
        .collect()
}

/// Per-tick player maintenance: eating progress, vitals (regen, starvation,
/// drowning). Damage routes through merge like combat; deaths produce drops +
/// Died via commit.
fn tick_players(state: &mut TickState, changes: &mut ChangeSet, intents: &mut Vec<Intent>) {
    let difficulty = state.difficulty;
    let natural_regeneration = crate::command_rules::boolean(state, "natural_health_regeneration");
    crate::statistics::tick(state, state.tick + 1);
    // Snapshot player entities to avoid borrow issues.
    let players = live_players(state);
    for (_, entity, connection) in players {
        if state.session_locked(connection) {
            continue;
        }
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        elytra::tick(state, entity);
        // Eating progress.
        let consumable_refused = state
            .players
            .get(&entity)
            .and_then(|player| player.finishing_consumable_debit())
            .is_some_and(|debit| {
                crate::plugin_inventories::below_player_debit_floors(state, entity, &[debit], None)
            });
        let consumable_event = (!consumable_refused)
            .then(|| {
                state.players.get(&entity).and_then(|player| {
                    player.finishing_consumable_event(&state.registry.item_rules())
                })
            })
            .flatten();
        let effects = if consumable_refused {
            crate::player::PlayerEffects::default()
        } else {
            let rules = state.registry.item_rules();
            state
                .players
                .get_mut(&entity)
                .map(|player| player.tick_item_use(&rules))
                .unwrap_or_default()
        };
        if let Some(event) = consumable_event {
            let pos = state.entities.pos(slot);
            crate::vibration::emit(state, pos, event, Some(entity));
        }
        if let Some(item) = &effects.consumed_item {
            let item = crate::advancements::item_snapshot(state, item);
            crate::advancements::trigger(
                state,
                entity,
                "minecraft:consume_item",
                serde_json::json!({"item":item}),
            );
            crate::effects::sync_appearance(state, entity);
        }
        // Apply eating effects inline (drops spawn immediately).
        if effects.chorus_consumed {
            crate::consumables::finish_chorus(state, entity);
        }
        for change in effects.changes {
            state.push_player(connection, change);
        }
        let pos = state.entities.pos(slot);
        for stack in effects.dropped {
            let id = spawn_item_entity(state, pos, stack);
            changes.entities_spawned.push(id);
        }
        // Eye-in-water for drowning.
        let eye = [
            mc_physics::mth::floor_i32(pos[0]),
            mc_physics::mth::floor_i32(pos[1] + 1.62),
            mc_physics::mth::floor_i32(pos[2]),
        ];
        let eye_in_water = state
            .world
            .block_at(eye[0], eye[1], eye[2])
            .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
            .is_some_and(|block| {
                block.block.id.path() == "water"
                    || block
                        .properties
                        .iter()
                        .any(|(name, value)| name == "waterlogged" && value == "true")
            });
        let vitals_tick = state
            .players
            .get_mut(&entity)
            .map(|player| {
                let can_drown = !player.creative()
                    && !player.spectator()
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::WaterBreathing)
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::ConduitPower)
                    && !player
                        .active_effects
                        .has(crate::effects::EffectId::BreathOfTheNautilus);
                let consume_air = !eye_in_water
                    || !can_drown
                    || crate::enchantments::consumes_air(
                        &player.inventory.slots[39],
                        &mut player.enchantment_seed,
                    );
                player.vitals.tick(
                    difficulty,
                    eye_in_water,
                    can_drown,
                    consume_air,
                    natural_regeneration,
                )
            })
            .unwrap_or_default();
        if vitals_tick.air_changed
            && let Some(player) = state.players.get(&entity)
        {
            let change = player.air_change();
            state.push_player(connection, change);
        }
        // Vitals damage through merge (footprinted on the player).
        for (kind, amount) in vitals_tick.damage {
            let seq = state.next_seq(entity);
            intents.push(Intent {
                actor: entity,
                seq,
                kind: IntentKind::DamageEntity {
                    target: entity,
                    amount,
                    kind,
                    source: None,
                },
                footprint: Footprint {
                    entity_reads: vec![entity],
                    entity_writes: vec![entity],
                    ..Default::default()
                },
            });
        }
        if vitals_tick.health_changed {
            let (health, food, saturation) = state
                .players
                .get(&entity)
                .map(|player| {
                    (
                        player.vitals.health,
                        player.vitals.food,
                        player.vitals.saturation,
                    )
                })
                .unwrap_or((20.0, 20, 5.0));
            state.push_player(
                connection,
                PlayerChange::Health {
                    health,
                    food: food.clamp(0, 20) as u8,
                    saturation,
                },
            );
        }
    }
}

/// Hot worker snapshot; rare state is read by slot while the store is immutable.
#[derive(Debug, Clone, Copy)]
struct ReadRow {
    slot: EntitySlot,
    id: EntityId,
    /// Current column's ID-staggered activation decision.
    think: bool,
    pos: [f64; 3],
    panic_multiplier: f64,
    /// Velocity written last tick: the standing decision between AI thinks.
    vel: [f64; 3],
    rotation: [f32; 3],
    movement: crate::blueprint::MovementClass,
    air_supply: i32,
    horizontal_impulse: [f64; 2],
    alive: bool,
    ground: bool,
    half_width: f64,
    height: f64,
    sight_range: f32,
    attack_reach: f32,
    attack_damage: f32,
    melee_bonus: f32,
    attack_cooldown: u32,
    love: u32,
    is_enderman: bool,
    is_iron_golem: bool,
    is_slime_size_one: bool,
    /// Blueprint locomotion capabilities read by the motion step.
    climbs_walls: bool,
    aquatic_control: crate::species::AquaticControl,
    /// Resolved from the blueprint's goal set (data, not type names).
    chaser: bool,
    /// Breed goal, eligibility, adulthood, cooldown and love checked once.
    breed_ready: bool,
    /// Protocol type: partners must share it.
    type_id: u32,
    /// Mature item snapshot (item kind, alive, zero pickup delay).
    pickup_ready: bool,
    item_entity: bool,
    player: bool,
    speed: f32,
    enderman_target: Option<EntityId>,
    levitation: f32,
    slow_falling: bool,
    special: Option<crate::hostile::Special>,
    /// Ignores players by day (vanilla spider).
    night_aggro: bool,
    /// A lit fuse holds the mob still.
    standing: bool,
    plugin_target: Option<[f64; 3]>,
    leash_follow: bool,
    gold_target: Option<[f64; 3]>,
}

/// Owned movement write from one worker.
struct MoveWrite {
    slot: EntitySlot,
    pos: [f64; 3],
    vel: [f64; 3],
    rotation: [f32; 3],
    air_supply: i32,
    ground: bool,
    horizontal_impulse: [f64; 2],
}

#[derive(Default)]
struct EntityPhaseScratch {
    rows: Vec<ReadRow>,
    writes: Vec<Option<(MoveWrite, Vec<RawIntent>)>>,
    specials: Vec<(EntityId, crate::hostile::Special)>,
    players: Vec<(EntityId, [f64; 3])>,
    activation: HashMap<(i32, i32), u64>,
    periods: Vec<u8>,
    resting: Vec<EntitySlot>,
}

fn unchanged_entity_footprint(
    pos: [f64; 3],
    half_width: f64,
    chunk_revision: impl Fn(i32, i32) -> u64,
    unchanged_since: u64,
) -> bool {
    let half = half_width + 0.001;
    let chunk = |v: f64| mc_physics::mth::floor_i32(v).div_euclid(16);
    let (x0, x1) = (chunk(pos[0] - half), chunk(pos[0] + half));
    let (z0, z1) = (chunk(pos[2] - half), chunk(pos[2] + half));
    (x0..=x1).all(|cx| (z0..=z1).all(|cz| chunk_revision(cx, cz) <= unchanged_since))
}

fn motion_thinks(tick: u64, flags: u16, period: u8) -> bool {
    let phase = ((flags & MOTION_PHASE_MASK) >> MOTION_PHASE_SHIFT) as u8;
    (tick as u8).wrapping_add(phase) & (period - 1) == 0
}

/// Rest before the wide snapshot. Decision ticks additionally prove that the
/// final wander branch's complete motion step is bit-identical to the input.
fn early_entity_rest(
    view: &PrepView,
    slot: EntitySlot,
    period: u8,
    unchanged_since: u64,
    special_idle: bool,
    targets_empty: bool,
) -> bool {
    let flags = view.entities.motion_flags(slot);
    if flags & MOTION_REST == 0
        || !special_idle
        || (motion_thinks(view.tick, flags, period)
            && flags & crate::entities::MOTION_WANDER_REST == 0
            && !(targets_empty && flags & crate::entities::MOTION_TARGETLESS_WANDER_REST != 0))
        || !view.ground[slot.0 as usize]
        || view.velocities[slot.0 as usize] != [0.0; 3]
    {
        return false;
    }
    let body = &view.bodies[slot.0 as usize];
    if body
        .hurt_at
        .is_some_and(|at| view.tick.saturating_sub(at) < 100)
    {
        return false;
    }
    if motion_thinks(view.tick, flags, period) {
        // The proof samples current geometry, so unrelated edits anywhere in
        // the footprint's chunk need not reject an otherwise exact no-op.
        return idle_wander_motion_unchanged(view, slot);
    }
    unchanged_entity_footprint(
        view.positions[slot.0 as usize],
        body.half_width,
        |cx, cz| view.world.chunk_revision(cx, cz),
        unchanged_since,
    )
}

/// Keep the same physics, breathing and effect integration as `step_entity`.
/// In particular, an imported on-ground flag is not itself a support proof.
fn idle_wander_motion_unchanged(view: &PrepView, slot: EntitySlot) -> bool {
    let body = &view.bodies[slot.0 as usize];
    let row = view.entities.row(slot);
    if crate::entity_motion::wander(body.movement, body.id.0, view.tick, row.speed) != [0.0; 3] {
        return false;
    }
    let pos = view.positions[slot.0 as usize];
    let vel = view.velocities[slot.0 as usize];
    let rotation = view.rotations[slot.0 as usize];
    let physics_body = mc_physics::EntityBody {
        position: mc_physics::Vec3::new(pos[0], pos[1], pos[2]),
        velocity: mc_physics::Vec3::new(vel[0], vel[1], vel[2]),
        aabb: mc_physics::Aabb {
            half_width: body.half_width,
            height: body.height,
        },
        on_ground: view.ground[slot.0 as usize],
    };
    let reader = view.world.physics_reader();
    let sampler = WorkerSampler {
        block_at: &|x, y, z| reader.block_at(x, y, z),
        registry: view.registry,
    };
    let capabilities = match body.kind {
        EntityKind::Mob { type_id } => view
            .registry
            .blueprint(type_id)
            .map(|blueprint| blueprint.capabilities)
            .unwrap_or_default(),
        _ => return false,
    };
    let effects =
        (view.entities.motion_flags(slot) & MOTION_EFFECTS != 0).then_some(&row.active_effects);
    let result = crate::entity_motion::step(
        crate::entity_motion::MotionInput {
            body: physics_body,
            movement: body.movement,
            drive: [0.0; 3],
            impulse: view.impulses[slot.0 as usize],
            rotation,
            look_target: None,
            climbs_walls: capabilities.climbs_walls,
            aquatic_control: capabilities.aquatic,
            air_supply: body.air_supply,
            levitation: effects.map_or(0.0, |effects| {
                effects.level(crate::effects::EffectId::Levitation)
            }),
            slow_falling: effects
                .is_some_and(|effects| effects.has(crate::effects::EffectId::SlowFalling)),
            id: body.id.0,
            tick: view.tick,
        },
        &sampler,
        |x, z| view.world.has_chunk(x.div_euclid(16), z.div_euclid(16)),
    );
    result.body.position.x.to_bits() == pos[0].to_bits()
        && result.body.position.y.to_bits() == pos[1].to_bits()
        && result.body.position.z.to_bits() == pos[2].to_bits()
        && result.body.velocity.x.to_bits() == vel[0].to_bits()
        && result.body.velocity.y.to_bits() == vel[1].to_bits()
        && result.body.velocity.z.to_bits() == vel[2].to_bits()
        && result.body.on_ground == physics_body.on_ground
        && result.rotation.map(f32::to_bits) == rotation.map(f32::to_bits)
        && result.impulse.map(f64::to_bits) == view.impulses[slot.0 as usize].map(f64::to_bits)
        && result.air_supply == body.air_supply
        && !result.drowning
}

/// Live, non-converting resident bodies (villager, wandering trader) in slot
/// order. The resident index replaces a dense scan; slot order is the
/// `TargetGrid` tie-break, so it must match the dense order exactly.
fn resident_villager_targets(entities: &EntityStore) -> Vec<(EntityId, [f64; 3])> {
    let mut slots = Vec::new();
    entities.resident_slots_sorted(&mut slots);
    slots
        .into_iter()
        .filter(|&slot| {
            let body = entities.body(slot);
            body.alive
                && entities
                    .row(slot)
                    .villager
                    .as_ref()
                    .is_some_and(|v| v.conversion_time.is_none())
        })
        .map(|slot| (entities.body(slot).id, entities.pos(slot)))
        .collect()
}

/// Prepare every admitted slot (`periods[slot] != 0`) into `out`, in slot
/// order. The parallel path admits disjoint ascending slot ranges over an
/// immutable view (specials, resting and the slots needing rows concatenate in
/// chunk order, which reproduces the serial order), then builds every row
/// straight into `out.rows` with an indexed parallel collect: no per-chunk row
/// vectors and no row copy. Rows needing the `TickState`-only helpers are then
/// rebuilt serially before the combined projection reduction.
fn prepare_rows(
    state: &TickState,
    periods: &[u8],
    env: &PrepEnv,
    parallel: bool,
    out: &mut PrepChunk,
) {
    // `out` arrives empty (entity-phase scratch is empty between ticks; only
    // its capacity is reused): the parallel collect replaces `out.rows`.
    debug_assert!(out.rows.is_empty() && out.rare.is_empty());
    let view = PrepView::of(state);
    if parallel {
        use rayon::prelude::*;
        #[derive(Default)]
        struct Admission {
            kept: Vec<(EntitySlot, u16)>,
            specials: Vec<(EntityId, crate::hostile::Special)>,
            resting: Vec<EntitySlot>,
        }
        let parts: Vec<Admission> = periods
            .par_chunks(PREP_CHUNK)
            .enumerate()
            .map(|(chunk, periods)| {
                let base = chunk * PREP_CHUNK;
                let mut part = Admission::default();
                for (offset, &period) in periods.iter().enumerate() {
                    if period != 0
                        && let Some(kept) = admit_slot(
                            &view,
                            base + offset,
                            period,
                            env,
                            &mut part.specials,
                            &mut part.resting,
                        )
                    {
                        part.kept.push(kept);
                    }
                }
                part
            })
            .collect();
        let mut kept = Vec::with_capacity(parts.iter().map(|part| part.kept.len()).sum());
        for part in parts {
            kept.extend(part.kept);
            out.specials.extend(part.specials);
            out.resting.extend(part.resting);
        }
        let mut is_rare = Vec::new();
        kept.par_iter()
            .map(|&(slot, flags)| {
                read_row(
                    &view,
                    slot,
                    flags,
                    periods[slot.0 as usize],
                    &RarePrep::default(),
                )
            })
            .unzip_into_vecs(&mut out.rows, &mut is_rare);
        out.rare.extend(
            is_rare
                .iter()
                .enumerate()
                .filter_map(|(index, &rare): (usize, &bool)| rare.then_some(index)),
        );
    } else {
        for (index, &period) in periods.iter().enumerate() {
            if period != 0 {
                prepare_slot(&view, index, period, env, out);
            }
        }
    }
    for index in std::mem::take(&mut out.rare) {
        let slot = out.rows[index].slot;
        let flags = view.entities.motion_flags(slot);
        let period = periods[slot.0 as usize];
        out.rows[index] = read_row(&view, slot, flags, period, &RarePrep::of(state, slot)).0;
    }
    // Rare helpers can change `chaser` (notably enderman anger), so project
    // only final rows. Chunk collection and concatenation retain row order.
    out.summary = if parallel {
        use rayon::prelude::*;
        let parts: Vec<_> = out
            .rows
            .par_chunks(PREP_CHUNK)
            .enumerate()
            .map(|(chunk, rows)| PrepSummary::of(rows, chunk * PREP_CHUNK))
            .collect();
        let mut summary = PrepSummary::default();
        for part in parts {
            summary.has_dolphin |= part.has_dolphin;
            for (total, count) in summary.consumers.iter_mut().zip(part.consumers) {
                *total += count;
            }
            for (cell, indices) in part.cow_cells {
                summary.cow_cells.entry(cell).or_default().extend(indices);
            }
        }
        summary
    } else {
        PrepSummary::of(&out.rows, 0)
    };
}

/// Slot ranges larger than this admission count prepare rows in parallel.
const PARALLEL_PREP_MIN: usize = 4096;
const PREP_CHUNK: usize = 2048;

/// Read-only components row preparation needs. `TickState` owns channel
/// endpoints and is not `Sync`, so workers borrow only these.
struct PrepView<'a> {
    entities: &'a EntityStore,
    bodies: &'a [Body],
    positions: &'a [[f64; 3]],
    velocities: &'a [[f64; 3]],
    rotations: &'a [[f32; 3]],
    ground: &'a [bool],
    impulses: &'a [[f64; 2]],
    registry: &'a crate::registry::CoreRegistry,
    world: &'a crate::world::ChunkWorld,
    hostile: &'a crate::hostile::HostileState,
    tick: u64,
}

impl<'a> PrepView<'a> {
    fn of(state: &'a TickState) -> Self {
        Self {
            entities: &state.entities,
            bodies: state.entities.bodies(),
            positions: state.entities.positions(),
            velocities: state.entities.velocities(),
            rotations: state.entities.rotations(),
            ground: state.entities.grounds(),
            impulses: state.entities.impulses(),
            registry: &state.registry,
            world: &state.world,
            hostile: &state.hostile,
            tick: state.tick,
        }
    }
}

/// Helper results that need the whole `TickState` (creaking observers,
/// enderman anger, plugin routes, piglin gold). Only rows passing the cheap
/// gates in `read_row` ever carry them; workers leave the default and the
/// serial fix-up pass rebuilds those rows.
#[derive(Default)]
struct RarePrep {
    observed: bool,
    enderman_target: Option<EntityId>,
    plugin_target: Option<[f64; 3]>,
    gold_target: Option<[f64; 3]>,
}

impl RarePrep {
    fn of(state: &TickState, slot: EntitySlot) -> Self {
        Self {
            observed: crate::creaking::observed(state, slot),
            enderman_target: crate::enderman::target(state, slot),
            plugin_target: crate::plugin_npcs::motion_target(state, slot),
            gold_target: crate::piglin::gold_target(state, slot).map(|(_, pos)| pos),
        }
    }
}

/// Prepared rows, serial special pass entries and resting slots of a slot
/// range, plus indexes (into `rows`) of rows that need the rare helpers.
#[derive(Default)]
struct PrepChunk {
    rows: Vec<ReadRow>,
    specials: Vec<(EntityId, crate::hostile::Special)>,
    resting: Vec<EntitySlot>,
    rare: Vec<usize>,
    summary: PrepSummary,
}

#[derive(Default, Debug, PartialEq, Eq)]
struct PrepSummary {
    has_dolphin: bool,
    consumers: [usize; 5],
    cow_cells: HashMap<(i32, i32), Vec<usize>>,
}

impl PrepSummary {
    fn of(rows: &[ReadRow], base: usize) -> Self {
        let mut summary = Self::default();
        for (offset, row) in rows.iter().enumerate() {
            summary.has_dolphin |= row.movement == crate::blueprint::MovementClass::Swimmer
                && crate::dolphin::is_dolphin(row.type_id);
            if row.chaser && row.alive {
                if crate::piglin::is_iron_golem_type(row.type_id) {
                    summary.consumers[3] += 1;
                } else if crate::piglin::is_piglin_type(row.type_id) {
                    summary.consumers[1] += 1;
                } else {
                    summary.consumers[0] += 1;
                }
                if crate::piglin::hunts_villagers_type(row.type_id) {
                    summary.consumers[2] += 1;
                }
                if crate::piglin::is_zoglin_type(row.type_id) {
                    summary.consumers[4] += 1;
                }
            }
            if row.breed_ready && row.alive {
                let cell = (
                    mc_physics::mth::floor_i32(row.pos[0]) / 4,
                    mc_physics::mth::floor_i32(row.pos[2]) / 4,
                );
                summary
                    .cow_cells
                    .entry(cell)
                    .or_default()
                    .push(base + offset);
            }
        }
        summary
    }
}

struct PrepEnv {
    targets_empty: bool,
    unchanged_since: u64,
}

/// Admit one slot: record its serial special entry and either rest it
/// (`None`) or return the slot and its motion flags for a worker row.
/// Reads the immutable store only; the caller owns ordering.
fn admit_slot(
    view: &PrepView,
    index: usize,
    period: u8,
    env: &PrepEnv,
    specials: &mut Vec<(EntityId, crate::hostile::Special)>,
    resting: &mut Vec<EntitySlot>,
) -> Option<(EntitySlot, u16)> {
    let PrepEnv {
        targets_empty,
        unchanged_since,
    } = *env;
    let slot = EntitySlot(index as u32);
    let flags = view.entities.motion_flags(slot);
    let mut special_idle = true;
    if flags & MOTION_SPECIAL != 0 {
        let hint = view.entities.motion_hint(slot);
        if let Some(special) = hint.special {
            let id = view.bodies[slot.0 as usize].id;
            special_idle = targets_empty
                && view
                    .hostile
                    .idle_without_targets(id, special, hint.swell_metadata_neutral);
            if !special_idle {
                specials.push((id, special));
            }
        }
    }
    if early_entity_rest(
        view,
        slot,
        period,
        unchanged_since,
        special_idle,
        targets_empty,
    ) {
        resting.push(slot);
        return None;
    }
    Some((slot, flags))
}

/// Serial admission: admit one slot and snapshot its worker row into `out`.
fn prepare_slot(view: &PrepView, index: usize, period: u8, env: &PrepEnv, out: &mut PrepChunk) {
    let Some((slot, flags)) = admit_slot(
        view,
        index,
        period,
        env,
        &mut out.specials,
        &mut out.resting,
    ) else {
        return;
    };
    let (row, rare) = read_row(view, slot, flags, period, &RarePrep::default());
    if rare {
        out.rare.push(out.rows.len());
    }
    out.rows.push(row);
}

/// The worker snapshot of one admitted slot, and whether the row needs the
/// `TickState`-only helpers in `RarePrep` (every helper below returns its
/// default when that is false).
fn read_row(
    view: &PrepView,
    slot: EntitySlot,
    flags: u16,
    period: u8,
    rare: &RarePrep,
) -> (ReadRow, bool) {
    let think = motion_thinks(view.tick, flags, period);
    let body = &view.bodies[slot.0 as usize];
    let row = view.entities.row(slot);
    let observed = rare.observed;
    // Snapshot predicates share one immutable blueprint lookup per mob.
    let blueprint = match body.kind {
        EntityKind::Mob { type_id } => view.registry.blueprint(type_id),
        _ => None,
    };
    let needs_rare = crate::enderman::needs_state(row)
        || crate::plugin_npcs::needs_state(row)
        || crate::creaking::needs_state(body)
        || crate::piglin::eligible(body, row);
    let (chaser, breeder, item_entity, player, special, night_aggro) = match body.kind {
        EntityKind::Mob { .. } => {
            let special = blueprint.and_then(crate::hostile::special_of);
            (
                blueprint.is_some_and(|blueprint| {
                    blueprint.has_goal(crate::blueprint::GoalKind::MeleeChase)
                }) || matches!(
                    special,
                    Some(crate::hostile::Special::Ranged | crate::hostile::Special::Swell)
                ),
                blueprint
                    .is_some_and(|blueprint| blueprint.has_goal(crate::blueprint::GoalKind::Breed)),
                false,
                false,
                special,
                blueprint.is_some_and(|blueprint| blueprint.capabilities.night_aggro_only),
            )
        }
        EntityKind::Item { .. } => (false, false, true, false, None, false),
        EntityKind::ExperienceOrb { .. } => (false, false, false, false, None, false),
        EntityKind::Player { .. } => (false, false, false, true, None, false),
    };
    // Kind classification and scalar combat/defense inputs are snapshotted
    // here so `step_entity` never touches the cold row for the moved fields.
    let is_enderman = crate::enderman::is_enderman(body);
    let type_id = match body.kind {
        EntityKind::Mob { type_id }
        | EntityKind::Item { type_id }
        | EntityKind::ExperienceOrb { type_id } => type_id,
        EntityKind::Player { .. } => u32::MAX,
    };
    let is_iron_golem = crate::piglin::is_iron_golem_type(type_id);
    let is_slime_size_one = row.loot.slime_size == Some(1)
        && matches!(body.kind, EntityKind::Mob { type_id }
            if crate::piglin::name(type_id) == "minecraft:slime");
    let has_effects = flags & MOTION_EFFECTS != 0;
    let chaser = chaser
        && !row.controls.no_ai
        && (!matches!(body.kind, EntityKind::Mob { type_id } if crate::piglin::is_piglin_type(type_id))
            || crate::entity_life::adult(row))
        && (!is_enderman || rare.enderman_target.is_some());
    let breeder = breeder && !row.controls.no_ai;
    let special = special.filter(|_| !row.controls.no_ai);
    let panic_multiplier = if (body.burn_ticks > 0
        || body
            .hurt_at
            .is_some_and(|at| view.tick.saturating_sub(at) < 100))
        && blueprint
            .is_some_and(|blueprint| blueprint.has_goal(crate::blueprint::GoalKind::FleeFire))
    {
        if body.burn_ticks > 0 { 2.0 } else { 1.25 }
    } else {
        0.0
    };
    let impulse = view.impulses[slot.0 as usize];
    let (speed, melee_bonus, levitation, slow_falling) = if has_effects {
        let effects = &row.active_effects;
        (
            row.speed
                * ((1.0 + 0.2 * effects.level(crate::effects::EffectId::Speed))
                    * (1.0 - 0.15 * effects.level(crate::effects::EffectId::Slowness)).max(0.0)),
            effects.melee_bonus(),
            effects.level(crate::effects::EffectId::Levitation),
            effects.has(crate::effects::EffectId::SlowFalling),
        )
    } else {
        (row.speed, 0.0, 0.0, false)
    };
    // Panic source stays a rare-branch worker read (see `step_entity`).
    let read = ReadRow {
        think,
        enderman_target: rare.enderman_target,
        plugin_target: rare.plugin_target,
        gold_target: rare.gold_target,
        panic_multiplier,
        slot,
        id: body.id,
        pos: view.positions[slot.0 as usize],
        vel: view.velocities[slot.0 as usize],
        rotation: view.rotations[slot.0 as usize],
        movement: body.movement,
        air_supply: body.air_supply,
        horizontal_impulse: if observed { [0.0; 2] } else { impulse },
        alive: body.alive,
        ground: view.ground[slot.0 as usize],
        half_width: body.half_width,
        height: body.height,
        sight_range: row.sight_range,
        attack_reach: row.attack_reach,
        attack_damage: row.attack_damage,
        melee_bonus,
        attack_cooldown: body.attack_cooldown,
        love: row.love,
        is_enderman,
        is_iron_golem,
        is_slime_size_one,
        climbs_walls: blueprint.is_some_and(|blueprint| blueprint.capabilities.climbs_walls),
        aquatic_control: blueprint.map_or(crate::species::AquaticControl::Other, |blueprint| {
            blueprint.capabilities.aquatic
        }),
        chaser,
        breed_ready: breeder
            && crate::entity_life::can_breed_in(view.registry, body, row)
            && row.entity_life.child_ticks == 0
            && row.breed_cooldown == 0
            && row.love > 0,
        type_id,
        pickup_ready: item_entity && body.alive && body.pickup_delay == 0,
        item_entity,
        player,
        speed,
        levitation,
        slow_falling,
        special,
        night_aggro,
        standing: (special == Some(crate::hostile::Special::Swell)
            && view.hostile.fuse(body.id) > 0)
            || matches!(row.special_mob.as_deref(), Some(crate::special_mobs::Retained::Wither { invulnerable }) if *invulnerable > 0)
            || (matches!(
                special,
                Some(crate::hostile::Special::Native(
                    crate::blueprint::GoalKind::Beam | crate::blueprint::GoalKind::ElderBeam
                ))
            ) && view
                .hostile
                .native
                .attacks
                .get(&body.id)
                .is_some_and(|attack| attack.stage > 0))
            || row.controls.no_ai
            || observed
            || blueprint.is_some_and(|blueprint| blueprint.goals.is_empty())
            || crate::taming::suppress_ai(row)
            || crate::warden::suppress_ai(row)
            || crate::piglin::admiring(row),
        leash_follow: row.entity_life.leash.is_some()
            && !row.controls.no_ai
            && !row.entity_life.sitting
            && row.entity_life.mount.rider.is_none(),
    };
    (read, needs_rare)
}

/// Entity phase: every simulated entity reads the snapshot and emits its own
/// next state plus intents. Parallel by data over scoped threads; workers
/// return owned writes applied in slot order afterwards. No locks: the only
/// synchronization is the scope join.
fn run_entity_phase(state: &mut TickState, sim: bool) -> Vec<Intent> {
    if sim {
        crate::contact_hazards::hurt_by_water(state);
        crate::enderman::tick(state);
    }
    let prep_start = std::time::Instant::now();
    state.entities.prepare_motion_hints(&state.registry);
    let unchanged_since = state.entity_phase_revision;
    state.entity_phase_revision = state.world.revision_now();
    let EntityPhaseScratch {
        mut rows,
        writes: mut ordered,
        mut specials,
        mut players,
        mut activation,
        mut periods,
        mut resting,
    } = std::mem::take(&mut state.entity_phase_scratch);
    periods.resize(state.entities.len(), 0);
    let mut admitted = 0;
    // Compact flags preserve synthetic player bodies without PlayerState,
    // including global motion admission for players outside active chunks.
    for slot in state.entities.motion_slots(MOTION_OWNED | MOTION_PLAYER) {
        periods[slot.0 as usize] = 1;
        admitted += 1;
        if state.entities.motion_flags(slot) & MOTION_ALIVE != 0 {
            players.push((state.entities.body(slot).id, state.entities.pos(slot)));
        }
    }
    // Mob decisions run every 1/2/4 ticks at near/mid/far chunk distances.
    for (_, pos) in &players {
        let cx = mc_physics::mth::floor_i32(pos[0]).div_euclid(16);
        let cz = mc_physics::mth::floor_i32(pos[2]).div_euclid(16);
        for dz in -MOB_MID_CHUNKS..=MOB_MID_CHUNKS {
            for dx in -MOB_MID_CHUNKS..=MOB_MID_CHUNKS {
                let period = if dx.abs().max(dz.abs()) <= MOB_NEAR_CHUNKS {
                    1
                } else {
                    2
                };
                activation
                    .entry((cx + dx, cz + dz))
                    .and_modify(|best| *best = (*best).min(period))
                    .or_insert(period);
            }
        }
    }
    // Exact, mostly active membership lets hot flags/positions replace per-ID
    // index traversal even when a few occupied chunks are inactive.
    if !sim {
        // Frozen (`/tick freeze`): only player bodies step; every other
        // entity keeps its position, velocity and timers.
    } else if prefer_dense_entity_admission(state) {
        let mut previous: Option<(ChunkPos, u8)> = None;
        for slot in state.entities.motion_slots(MOTION_OWNED) {
            if state.entities.motion_flags(slot) & MOTION_PLAYER != 0 {
                continue;
            }
            let chunk = state.entities.chunk_of(slot);
            let period = match previous {
                Some((last, period)) if last == chunk => period,
                _ => {
                    let period = if state.world.is_active_position(chunk.x, chunk.z) {
                        activation
                            .get(&(chunk.x, chunk.z))
                            .copied()
                            .unwrap_or(MOB_FAR_PERIOD) as u8
                    } else {
                        0
                    };
                    previous = Some((chunk, period));
                    period
                }
            };
            if period != 0 {
                periods[slot.0 as usize] = period;
                admitted += 1;
            }
        }
    } else {
        // Preserve the indexed union and period order for old type-transition
        // entries rather than silently replacing them with current positions.
        for chunk in state.world.active_positions() {
            if !state.world.is_active(chunk.x, chunk.z) {
                continue;
            }
            let period = activation
                .get(&(chunk.x, chunk.z))
                .copied()
                .unwrap_or(MOB_FAR_PERIOD) as u8;
            state.entities.for_each_slot_in_chunk(chunk, |slot| {
                let flags = state.entities.motion_flags(slot);
                if flags & MOTION_OWNED != 0 && flags & MOTION_PLAYER == 0 {
                    let mark = &mut periods[slot.0 as usize];
                    admitted += usize::from(*mark == 0);
                    *mark = period;
                }
            });
        }
    }
    let targets_empty = !crate::hostile::has_live_targets(state);
    rows.reserve(admitted);
    let mut prepared = PrepChunk {
        rows: std::mem::take(&mut rows),
        specials: std::mem::take(&mut specials),
        resting: std::mem::take(&mut resting),
        rare: Vec::new(),
        summary: PrepSummary::default(),
    };
    prepare_rows(
        state,
        &periods,
        &PrepEnv {
            targets_empty,
            unchanged_since,
        },
        admitted >= PARALLEL_PREP_MIN,
        &mut prepared,
    );
    let PrepSummary {
        has_dolphin,
        consumers,
        cow_cells,
    } = std::mem::take(&mut prepared.summary);
    PrepChunk {
        rows,
        specials,
        resting,
        ..
    } = prepared;
    if rows.is_empty() && resting.is_empty() {
        periods.clear();
        activation.clear();
        state.entity_phase_scratch = EntityPhaseScratch {
            rows,
            writes: ordered,
            specials,
            players,
            activation,
            periods,
            resting,
        };
        return Vec::new();
    }
    let golem_targets = if crate::piglin::iron_golem_present(state) {
        crate::villager::defense::targets(state)
    } else {
        Vec::new()
    };
    let world = &state.world;
    let registry = &state.registry;
    let chunk_loaded = |x: i32, z: i32| world.has_chunk(x.div_euclid(16), z.div_euclid(16));
    let tick = state.tick;
    let chunk_revision = |cx: i32, cz: i32| world.chunk_revision(cx, cz);
    let rows_ref = &rows;
    let dolphin_toys: Vec<[f64; 3]> = if has_dolphin {
        rows.iter()
            .filter(|row| row.item_entity && row.alive && row.pickup_ready)
            .map(|row| row.pos)
            .collect()
    } else {
        Vec::new()
    };
    // Chase targets: vanilla monsters ignore creative and spectator players.
    let targets: Vec<(EntityId, [f64; 3])> = players
        .iter()
        .filter(|(id, _)| {
            state.players.get(id).is_some_and(|player| {
                !player.creative()
                    && !player.spectator()
                    && !state.session_locked(player.connection)
            })
        })
        .copied()
        .collect();
    let piglin_targets: Vec<_> = if crate::piglin::piglin_present(state) {
        targets
            .iter()
            .filter(|(id, _)| !crate::piglin::wears_gold(state, *id))
            .copied()
            .collect()
    } else {
        Vec::new()
    };
    let zoglin_targets: Vec<_> = if crate::piglin::zoglin_present(state) {
        state
            .entities
            .slots()
            .filter_map(|slot| {
                let body = state.entities.body(slot);
                let EntityKind::Mob { type_id } = body.kind else {
                    return None;
                };
                (body.alive
                    && state.entities.row(slot).health > 0.0
                    && mc_data::entity_types::entity_type_contract_26_1_2_by_protocol_id(type_id)
                        .is_some_and(|contract| {
                            contract.behavior.archetype
                                == mc_data::entity_types::EntityArchetype::Living
                        })
                    && !matches!(
                        crate::piglin::name(type_id),
                        "minecraft:zoglin" | "minecraft:creeper" | "minecraft:armor_stand"
                    ))
                .then(|| (body.id, state.entities.pos(slot)))
            })
            .collect()
    } else {
        Vec::new()
    };
    let villagers = resident_villager_targets(&state.entities);
    // Chase searches per class this tick (over-counted: not every chaser
    // thinks). Only a class with many consumers pays for an index; the rest
    // scan linearly as before (see target_grid.rs for boundary and fence).
    let grid = crate::target_grid::TargetGrid::for_consumers;
    let player_grid = grid(&targets, consumers[0]);
    let piglin_grid = grid(&piglin_targets, consumers[1]);
    let villager_grid = grid(&villagers, consumers[2]);
    let golem_grid = grid(&golem_targets, consumers[3]);
    let zoglin_grid = grid(&zoglin_targets, consumers[4]);
    let cows_ref = &cow_cells;
    let scope_start = std::time::Instant::now();
    // Work-stealing over the core's long-lived rayon workers (no threads
    // spawned per tick); `collect` keeps slot order.
    let ctx = StepCtx {
        rows: rows_ref,
        entities: &state.entities,
        players: player_grid,
        dolphin_toys: &dolphin_toys,
        villagers: villager_grid,
        golem_targets: golem_grid,
        piglin_targets: piglin_grid,
        zoglin_targets: zoglin_grid,
        cow_cells: cows_ref,
        world,
        chunk_loaded: &chunk_loaded,
        registry,
        tick,
        chunk_revision: &chunk_revision,
        unchanged_since,
        difficulty: state.difficulty,
        night: crate::natural_spawn::is_night(state.day_time),
    };
    // Preserve Rayon's indexed collection: filtering first falls back to
    // per-worker vectors and a linked-list merge. Empty rows stay in place
    // until the serial drain, which also preserves slot order without a sort.
    {
        use rayon::prelude::*;
        rows.par_iter()
            .with_min_len(256)
            .map(|row| row.alive.then(|| step_entity(*row, &ctx)))
            .collect_into_vec(&mut ordered);
    }
    let scope_time = scope_start.elapsed();
    let prep_time = scope_start.duration_since(prep_start);
    let commit_start = std::time::Instant::now();
    let resting_count = resting.len();
    let mut seq = std::mem::take(&mut state.seq_counter);
    let mut intents = Vec::new();
    let physical_profiles = state.registry.item_facts.has_physical_profiles();
    for slot in resting.drain(..) {
        // A quiet row's writeback is a no-op; the hot flag spares the cold
        // row (a cache miss per resting body).
        if state.entities.motion_flags(slot) & crate::entities::MOTION_QUIET_TIMERS != 0 {
            continue;
        }
        let body = state.entities.body(slot);
        state.entities.set_motion_state(
            slot,
            [0.0; 2],
            body.air_supply,
            body.pickup_delay.saturating_sub(1),
            body.attack_cooldown.saturating_sub(1),
        );
    }
    for (write, mut raw) in ordered.drain(..).flatten() {
        let attack_cooldown = raw
            .iter()
            .any(|intent| {
                matches!(
                    intent.kind,
                    IntentKind::DamageEntity {
                        kind: crate::survival::DamageKind::Melee,
                        ..
                    }
                )
            })
            .then_some(MELEE_ATTACK_INTERVAL);
        let was_ground = state.entities.ground(write.slot);
        let old = state.entities.apply_motion_result(
            write.slot,
            &crate::entities::MotionResult {
                pos: write.pos,
                vel: write.vel,
                yaw: write.rotation[0],
                pitch: write.rotation[1],
                ground: write.ground,
                horizontal_impulse: write.horizontal_impulse,
                air_supply: write.air_supply,
                attack_cooldown,
            },
        );
        if matches!(
            state.entities.body(write.slot).kind,
            EntityKind::Item { .. }
        ) {
            crate::redstone::containers::item_moved(&mut state.redstone_containers, write.pos);
        }
        let fall = crate::mob_fall::after_move(
            state,
            write.slot,
            old,
            write.pos,
            was_ground,
            write.ground,
        );
        if fall > 0.0 {
            let actor = state.entities.body(write.slot).id;
            raw.push(RawIntent {
                actor,
                kind: IntentKind::DamageEntity {
                    target: actor,
                    amount: fall,
                    kind: crate::survival::DamageKind::Fall,
                    source: None,
                },
                footprint: Footprint {
                    entity_writes: vec![actor],
                    ..Default::default()
                },
            });
        }
        // Only a player or a body wearing armor can spend movement stamina;
        // the hot flags answer that without dereferencing the row's
        // appearance (a cache miss per unarmored mob).
        if physical_profiles
            && state.entities.motion_flags(write.slot)
                & (crate::entities::MOTION_PLAYER | crate::entities::MOTION_ARMORED)
                != 0
        {
            crate::physical_defense::accepted_movement(
                state,
                write.slot,
                mth::length2(write.pos[0] - old[0], write.pos[2] - old[2]),
            );
        }
        for raw_intent in raw {
            let next = seq.get(&raw_intent.actor).copied().unwrap_or(0);
            seq.insert(raw_intent.actor, next + 1);
            intents.push(Intent {
                actor: raw_intent.actor,
                seq: next,
                kind: raw_intent.kind,
                footprint: raw_intent.footprint,
            });
        }
    }
    state.seq_counter = seq;
    crate::item_motion::merge_neighbours(state);
    let commit_time = commit_start.elapsed();
    let post_start = std::time::Instant::now();
    // Pickup intents: players collect mature item entities in reach.
    // Exact kind lanes, authoritative maturity and post-move positions.
    if sim {
        collect_pickup_intents(state, &mut intents);
    }
    // Shooting skeletons, flying arrows, swelling creepers (serial: they
    // read the world and every entity).
    if sim {
        crate::hostile::run(state, &mut intents, &specials);
        crate::projectiles::run(state, &mut intents, &specials);
    }
    if std::env::var("MC_CORE_PHASE_LOG").is_ok() {
        eprintln!(
            "  entity detail ms: scope={:.1} n={} prep={:.1} commit={:.1} post={:.1} rest={}",
            scope_time.as_secs_f64() * 1000.0,
            rows.len(),
            prep_time.as_secs_f64() * 1000.0,
            commit_time.as_secs_f64() * 1000.0,
            post_start.elapsed().as_secs_f64() * 1000.0,
            resting_count,
        );
    }
    rows.clear();
    specials.clear();
    players.clear();
    activation.clear();
    periods.clear();
    state.entity_phase_scratch = EntityPhaseScratch {
        rows,
        writes: ordered,
        specials,
        players,
        activation,
        periods,
        resting,
    };
    let _ = tick;
    intents
}

/// Shared flags bit of a sneaking entity.
pub(crate) const SNEAKING_FLAG: u8 = 0x02;
/// Vanilla `Pose.CROUCHING` ordinal.
const CROUCHING_POSE: u8 = 5;

/// Ticks between a mob's melee hits (vanilla `MeleeAttackGoal`).
const MELEE_ATTACK_INTERVAL: u32 = 20;

/// Intent without a sequence number (assigned at drain time).
struct RawIntent {
    actor: EntityId,
    kind: IntentKind,
    footprint: Footprint,
}

/// Step one entity: returns its movement write plus raw intents.
/// Shared read-only context for entity stepping (no locks, no copies).
struct StepCtx<'a> {
    rows: &'a [ReadRow],
    /// Immutable through the indexed worker join; writes/compaction follow it.
    entities: &'a crate::entities::EntityStore,
    players: crate::target_grid::TargetGrid<'a>,
    dolphin_toys: &'a [[f64; 3]],
    villagers: crate::target_grid::TargetGrid<'a>,
    golem_targets: crate::target_grid::TargetGrid<'a>,
    piglin_targets: crate::target_grid::TargetGrid<'a>,
    zoglin_targets: crate::target_grid::TargetGrid<'a>,
    cow_cells: &'a HashMap<(i32, i32), Vec<usize>>,
    world: &'a ChunkWorld,
    chunk_loaded: &'a (dyn Fn(i32, i32) -> bool + Sync),
    registry: &'a CoreRegistry,
    tick: u64,
    /// Content revision of a resident chunk (`u64::MAX` when absent).
    chunk_revision: &'a (dyn Fn(i32, i32) -> u64 + Sync),
    /// World revision the previous entity phase read.
    unchanged_since: u64,
    difficulty: crate::survival::Difficulty,
    night: bool,
}

fn step_entity(row: ReadRow, ctx: &StepCtx) -> (MoveWrite, Vec<RawIntent>) {
    let rows = ctx.rows;
    let players = ctx.players.table();
    let cow_cells = ctx.cow_cells;
    let reader = ctx.world.physics_reader();
    let block_at = &|x, y, z| reader.block_at(x, y, z);
    let chunk_loaded = ctx.chunk_loaded;
    let registry = ctx.registry;
    let tick = ctx.tick;
    let mut intents = Vec::new();
    if row.item_entity {
        let sampler = WorkerSampler { block_at, registry };
        let body = crate::item_motion::step(
            mc_physics::EntityBody {
                position: mc_physics::Vec3::new(row.pos[0], row.pos[1], row.pos[2]),
                velocity: mc_physics::Vec3::new(row.vel[0], row.vel[1], row.vel[2]),
                aabb: mc_physics::Aabb {
                    half_width: row.half_width,
                    height: row.height,
                },
                on_ground: row.ground,
            },
            &sampler,
        );
        return (
            MoveWrite {
                slot: row.slot,
                pos: [body.position.x, body.position.y, body.position.z],
                vel: [body.velocity.x, body.velocity.y, body.velocity.z],
                ground: body.on_ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    if row.special == Some(crate::hostile::Special::Projectile) {
        // Arrows fly in `hostile::run`: they read the world and every entity.
        return (
            MoveWrite {
                slot: row.slot,
                pos: row.pos,
                vel: row.vel,
                ground: row.ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    if row.player {
        return (
            MoveWrite {
                slot: row.slot,
                pos: row.pos,
                vel: [0.0; 3],
                ground: row.ground,
                horizontal_impulse: [0.0; 2],
                rotation: row.rotation,
                air_supply: row.air_supply,
            },
            intents,
        );
    }
    // Mobs decide on their activation period, phase-shifted by id so each
    // tick carries an even share. Between decisions the last one keeps
    // executing (its horizontal velocity); physics and collision run every
    // tick either way.
    let think = row.think;
    let mut look_target = None;
    let mut vel = [0.0; 3];
    if let Some(target) = row.plugin_target {
        vel = crate::entity_motion::steer(row.movement, row.pos, target, f64::from(row.speed));
        look_target = Some(target);
    } else if row.standing {
        // Riding/sitting suppress decisions, never gravity or collision.
        if row.leash_follow {
            vel = row.vel;
        }
    } else if row.panic_multiplier > 0.0 {
        // Rare branch only: the one `last_player_hit` cold read on the
        // worker path.
        let panic_source = ctx
            .entities
            .row(row.slot)
            .last_player_hit
            .filter(|(_, at)| tick.saturating_sub(*at) < 100)
            .and_then(|(attacker, _)| ctx.entities.slot_of(attacker))
            .map(|attacker| ctx.entities.pos(attacker));
        vel = crate::entity_motion::panic(
            row.movement,
            row.id.0,
            tick,
            f64::from(row.speed) * row.panic_multiplier,
            row.pos,
            panic_source,
        );
    } else if !think {
        // A mob standing on the ground with no standing decision has nothing
        // to integrate while every chunk under its footprint is unchanged
        // since the last entity phase read it (a block edit, load or
        // eviction there wakes it this tick).
        if row.movement == crate::blueprint::MovementClass::Walker
            && row.ground
            && row.vel == [0.0; 3]
            && row.horizontal_impulse == [0.0; 2]
            && unchanged_entity_footprint(
                row.pos,
                row.half_width,
                ctx.chunk_revision,
                ctx.unchanged_since,
            )
        {
            return (
                MoveWrite {
                    slot: row.slot,
                    pos: row.pos,
                    vel: [0.0; 3],
                    ground: true,
                    horizontal_impulse: [0.0; 2],
                    rotation: row.rotation,
                    air_supply: row.air_supply,
                },
                intents,
            );
        }
        vel = [
            row.vel[0] - row.horizontal_impulse[0],
            row.vel[1],
            row.vel[2] - row.horizontal_impulse[1],
        ];
    } else if let Some(target) = row.gold_target {
        vel = crate::entity_motion::steer(row.movement, row.pos, target, f64::from(row.speed));
        look_target = Some(target);
    } else if crate::piglin::is_hoglin_type(row.type_id)
        && let Some(target) = crate::piglin::warped_fungus(row.pos, registry, block_at)
    {
        let away = [
            row.pos[0] * 2.0 - target[0],
            row.pos[1],
            row.pos[2] * 2.0 - target[2],
        ];
        vel = crate::entity_motion::steer(row.movement, row.pos, away, f64::from(row.speed));
    } else if row.chaser && !row.breed_ready {
        // Goals by priority: MeleeAttack, MeleeChase, Wander. Only chasers
        // need the nearest live player (compact table, no full-row scan).
        let mut nearest: crate::target_grid::Nearest = None;
        let sight = f64::from(if row.is_enderman {
            64.0
        } else {
            row.sight_range
        });
        let hunts_villagers = crate::piglin::hunts_villagers_type(row.type_id);
        let golem = row.is_iron_golem;
        let players = if crate::piglin::is_piglin_type(row.type_id) {
            &ctx.piglin_targets
        } else {
            &ctx.players
        };
        // Chain order (players, villagers, golem targets, zoglin targets)
        // and table order break distance ties, as one linear scan did.
        if !golem {
            players.consider(&mut nearest, row.pos, sight, |(id, _)| {
                row.enderman_target.is_none_or(|target| target == *id)
            });
        }
        if hunts_villagers {
            ctx.villagers
                .consider(&mut nearest, row.pos, sight, |_| true);
        }
        if golem {
            // Vanilla IronGolem.canAttackType: a player-created golem never
            // targets players, even ones it was hit by. Player creation is
            // rare cold state; read it only on this branch.
            let spared = matches!(
                ctx.entities.row(row.slot).special_mob.as_deref(),
                Some(crate::special_mobs::Retained::PlayerIronGolem)
            );
            ctx.golem_targets
                .consider(&mut nearest, row.pos, sight, |(id, _)| {
                    !(spared
                        && ctx.entities.slot_of(*id).is_some_and(|slot| {
                            matches!(ctx.entities.body(slot).kind, EntityKind::Player { .. })
                        }))
                });
        }
        if crate::piglin::is_zoglin_type(row.type_id) {
            ctx.zoglin_targets
                .consider(&mut nearest, row.pos, sight, |(id, _)| *id != row.id);
        }
        let mut acted = false;
        if let Some(((target, target_pos), dist)) = nearest
            && (!row.night_aggro || ctx.night)
        {
            look_target = Some(target_pos);
            if row.standing {
                // A creeper mid-fuse stands still.
                acted = true;
            } else if dist <= f64::from(row.attack_reach) {
                // In reach the mob stands and swings once per interval; a
                // bow user backs off from a close target instead.
                let ranged = matches!(
                    row.special,
                    Some(crate::hostile::Special::Ranged | crate::hostile::Special::Native(_))
                ) && !matches!(
                    row.special,
                    Some(crate::hostile::Special::Native(
                        crate::blueprint::GoalKind::SonicBoom | crate::blueprint::GoalKind::VexLife
                    ))
                );
                let attack_damage = if row.is_slime_size_one {
                    0.0
                } else {
                    (row.attack_damage + row.melee_bonus).max(0.0)
                };
                if !ranged && attack_damage > 0.0 && row.attack_cooldown == 0 {
                    intents.push(RawIntent {
                        actor: row.id,
                        kind: IntentKind::DamageEntity {
                            target,
                            amount: if golem {
                                attack_damage
                            } else {
                                ctx.difficulty.scale_mob_damage(attack_damage)
                            },
                            kind: crate::survival::DamageKind::Melee,
                            source: Some(crate::intent::DamageSource::Melee {
                                attacker: row.id,
                                position: row.pos,
                                yaw: 0.0,
                                bonus_knockback: 0.0,
                                sweep: false,
                                fire_ticks: 0,
                                bane_ticks: 0,
                            }),
                        },
                        footprint: Footprint {
                            entity_reads: vec![row.id],
                            entity_writes: vec![target],
                            ..Default::default()
                        },
                    });
                }
                if ranged && dist < crate::hostile::BOW_BACKOFF {
                    vel = crate::entity_motion::steer(
                        row.movement,
                        row.pos,
                        target_pos,
                        f64::from(row.speed) * crate::hostile::BACKOFF_SPEED,
                    )
                    .map(|value| -value);
                }
                acted = true;
            } else if dist > 0.5 {
                vel = crate::entity_motion::steer(
                    row.movement,
                    row.pos,
                    target_pos,
                    f64::from(row.speed),
                );
                acted = true;
            }
        }
        if !acted {
            vel = crate::entity_motion::wander(row.movement, row.id.0, tick, row.speed);
        }
    } else if row.breed_ready {
        // Goals by priority: Breed, Wander.
        let mut bred = false;
        let cell = (
            mc_physics::mth::floor_i32(row.pos[0]) / 4,
            mc_physics::mth::floor_i32(row.pos[2]) / 4,
        );
        let mut found: Option<&ReadRow> = None;
        'cells: for dx in -2..=2 {
            for dz in -2..=2 {
                let Some(members) = cow_cells.get(&(cell.0 + dx, cell.1 + dz)) else {
                    continue;
                };
                for index in members {
                    let other = &rows[*index];
                    if other.id == row.id || other.type_id != row.type_id {
                        continue;
                    }
                    let ox = other.pos[0] - row.pos[0];
                    let oy = other.pos[1] - row.pos[1];
                    let oz = other.pos[2] - row.pos[2];
                    if ox * ox + oy * oy + oz * oz < 64.0 {
                        found = Some(other);
                        break 'cells;
                    }
                }
            }
        }
        if let Some(other) = found {
            let delta = [
                other.pos[0] - row.pos[0],
                other.pos[1] - row.pos[1],
                other.pos[2] - row.pos[2],
            ];
            let distance = delta
                .into_iter()
                .map(|component| component * component)
                .sum::<f64>()
                .sqrt();
            look_target = Some(other.pos);
            vel = [
                delta[0] / distance.max(0.001) * f64::from(row.speed),
                0.0,
                delta[2] / distance.max(0.001) * f64::from(row.speed),
            ];
            bred = true;
            if distance < 3.0
                && row.id < other.id
                && row.love <= crate::entity_life::LOVE_TICKS - 60
                && other.love <= crate::entity_life::LOVE_TICKS - 60
                && let Some(child_type) = registry
                    .blueprint(row.type_id)
                    .and_then(|blueprint| blueprint.offspring)
            {
                let partner = other.id;
                intents.push(RawIntent {
                    actor: row.id,
                    kind: IntentKind::Breed {
                        partner,
                        child_pos: std::array::from_fn(|axis| {
                            (row.pos[axis] + other.pos[axis]) * 0.5
                        }),
                        child_type,
                    },
                    footprint: Footprint {
                        entity_reads: vec![row.id, partner],
                        entity_writes: vec![row.id, partner],
                        ..Default::default()
                    },
                });
            }
        }
        if !bred {
            vel = crate::entity_motion::wander(row.movement, row.id.0, tick, row.speed);
        }
    } else {
        vel = crate::entity_motion::wander(row.movement, row.id.0, tick, row.speed);
    }
    if !row.standing
        && crate::dolphin::is_dolphin(row.type_id)
        && let Some(drive) = crate::dolphin::drive(
            row.pos,
            row.air_supply,
            row.speed,
            players.iter().map(|(_, pos)| *pos),
            ctx.dolphin_toys.iter().copied(),
            block_at,
            registry,
        )
    {
        vel = drive;
    }
    let sampler = WorkerSampler { block_at, registry };
    let motion = crate::entity_motion::step(
        crate::entity_motion::MotionInput {
            body: mc_physics::EntityBody {
                position: mc_physics::Vec3::new(row.pos[0], row.pos[1], row.pos[2]),
                velocity: mc_physics::Vec3::new(row.vel[0], row.vel[1], row.vel[2]),
                aabb: mc_physics::Aabb {
                    half_width: row.half_width,
                    height: row.height,
                },
                on_ground: row.ground,
            },
            movement: row.movement,
            drive: vel,
            impulse: row.horizontal_impulse,
            rotation: row.rotation,
            look_target,
            climbs_walls: row.climbs_walls,
            aquatic_control: row.aquatic_control,
            air_supply: row.air_supply,
            levitation: row.levitation,
            slow_falling: row.slow_falling,
            id: row.id.0,
            tick,
        },
        &sampler,
        chunk_loaded,
    );
    if motion.drowning {
        intents.push(RawIntent {
            actor: row.id,
            kind: IntentKind::DamageEntity {
                target: row.id,
                amount: 2.0,
                kind: crate::survival::DamageKind::Drowning,
                source: None,
            },
            footprint: Footprint {
                entity_writes: vec![row.id],
                ..Default::default()
            },
        });
    }
    (
        MoveWrite {
            slot: row.slot,
            pos: [
                motion.body.position.x,
                motion.body.position.y,
                motion.body.position.z,
            ],
            vel: [
                motion.body.velocity.x,
                motion.body.velocity.y,
                motion.body.velocity.z,
            ],
            ground: motion.body.on_ground,
            horizontal_impulse: motion.impulse,
            rotation: motion.rotation,
            air_supply: motion.air_supply,
        },
        intents,
    )
}

/// Mob activation rings in chunks around each player (see the entity
/// phase): every tick within [`MOB_NEAR_CHUNKS`], every 2 ticks within
/// [`MOB_MID_CHUNKS`], every [`MOB_FAR_PERIOD`] ticks beyond.
const MOB_NEAR_CHUNKS: i32 = 2;
const MOB_MID_CHUNKS: i32 = 4;
const MOB_FAR_PERIOD: u64 = 4;

/// Item pickup intents: live players collect mature item entities in reach.
fn collect_pickup_intents(state: &TickState, intents: &mut Vec<Intent>) {
    collect_pickup_intents_from(
        state.entities.bodies(),
        state.entities.positions(),
        state
            .entities
            .player_slots()
            .filter(|slot| !state.entity_session_locked(state.entities.body(*slot).id)),
        state.entities.item_slots(),
        &state.seq_counter,
        intents,
    );
}

/// Inner pickup matcher over borrowed Body/position slices: exact kind lanes
/// (players, then items), authoritative maturity and post-move positions.
fn collect_pickup_intents_from(
    bodies: &[Body],
    positions: &[[f64; 3]],
    player_slots: impl Iterator<Item = EntitySlot>,
    item_slots: impl Iterator<Item = EntitySlot>,
    seq_counter: &HashMap<EntityId, u32>,
    intents: &mut Vec<Intent>,
) {
    let mut players: Vec<(EntityId, [f64; 3])> = Vec::new();
    let mut items: Vec<(EntityId, [f64; 3])> = Vec::new();
    for slot in player_slots {
        let body = &bodies[slot.0 as usize];
        if body.alive {
            players.push((body.id, positions[slot.0 as usize]));
        }
    }
    for slot in item_slots {
        let body = &bodies[slot.0 as usize];
        if body.alive && body.pickup_delay == 0 {
            items.push((body.id, positions[slot.0 as usize]));
        }
    }
    for (player, player_pos) in &players {
        for (item, item_pos) in &items {
            let dx = item_pos[0] - player_pos[0];
            let dy = item_pos[1] - player_pos[1];
            let dz = item_pos[2] - player_pos[2];
            // Vanilla Player.aiStep checks the player's box inflated by
            // (1, .5, 1), intersecting the item's .25-wide/.25-high box.
            if dx.abs() >= 1.425 || dz.abs() >= 1.425 || dy <= -0.75 || dy >= 2.3 {
                continue;
            }
            let next = seq_counter.get(player).copied().unwrap_or(0);
            intents.push(Intent {
                actor: *player,
                seq: next,
                kind: IntentKind::PickupItem {
                    item: *item,
                    count: i32::MAX,
                },
                footprint: Footprint {
                    entity_reads: vec![*player],
                    item_take: Some(*item),
                    ..Default::default()
                },
            });
            break;
        }
    }
}
