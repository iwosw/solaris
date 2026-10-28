//! Deterministic, bounded-memory 26.1.2 protocol players for the player-scale backend.
//! Stdout is exclusively newline JSON; diagnostics belong on stderr.
//! Optional open-loop input: `--open-loop --schedule-file /absolute/windows.json`.
//! The strict JSON is `{"windows":[{"name":"steady","duration_ms":10000,
//! "movement_hz":20,"edit_hz":1,"chat_hz":1,"command_hz":1,"commands":["say load"],
//! "recovery":false},{"name":"recovery","duration_ms":5000,"recovery":true}]}`.
//! Rates are integer slots/second (movement/edits per bot, commands on bot 0).
//! Stdin accepts newline JSON `{"event":"run"}` after ready, then `{"event":"stop"}`.
//! Run establishes one shared monotonic epoch; no queued catch-up or gameplay
//! before Run, during recovery, or after the schedule. SIGINT remains supported.
//! Admission/write receipts are not mechanic completion; use observed work.
//! `--unassisted-login` removes setup admission/creative/tp/item assistance;
//! Unassisted clients first rendezvous, then a shared watch releases every
//! login at `login_release.unix_ns` (distinct from the gameplay Run epoch).
//! `--auth-password PW` answers solaris-auth's register/login prompt right after Play
//! entry (before any assistance) and waits for the plugin's unlock reply.
//! `--verify-platform` checks five freshly decoded grass/clearance columns.
//! Setup commands emit `fixture_commands_sent` after all writes and
//! `fixture_setup_complete` after a unique input-ordered own chat echo.
//! Assisted open-loop Ready waits for setup and warm-write fences; only
//! fresh physical probes prove geometry, not command writes or chat fences.
//! Mechanic receipts: `--mechanic-spec FILE` declares bounded physical cells,
//! entity kinds, menus and finite public actions. `--mechanic-read-only` preserves
//! login state and only reads assigned menus; it never repairs the fixture.
//! Run rebases current received mechanic images and starts window-only work;
//! SetCenterChunk receipts prove cache-center changes, never per-move ACKs.
//! MIX-08 actions `reader_rate_limit` (`bot`, positive `bytes_per_second`) and
//! `restore_reader` use ordinary window/at_ms slots. A bounded Tokio relay limits
//! actual socket reads; mechanics.players records per-window receive progress and
//! final entity identities visible to each reader.
#![recursion_limit = "256"]

use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail, ensure};
use clap::Parser;
use mc_data::item_stack::ItemStack;
use mc_protocol::RawFrame;
use mc_protocol::codec::ReadMc;
use mc_protocol::packets::play::*;
use mc_protocol::packets::{ChatVisibility, ClientInformation, MainHand, Packet, ParticleStatus};
use mc_vanilla::wire_client::Client;
use parking_lot::Mutex as MechanicMutex;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::{Barrier, Semaphore, mpsc, watch};
use tokio::task::JoinSet;
use uuid::Uuid;
include!("player_scale_bots_mechanics.rs");
include!("player_scale_bots_setup.rs");
include!("player_scale_bots_auth.rs");
include!("player_scale_bots_reader.rs");
include!("player_scale_bots_streets.rs");
#[path = "player_scale_bots_loader.rs"]
mod loader;
#[path = "player_scale_bots_native.rs"]
mod native_probe;

const MIN_Y: i32 = -64;
const HEIGHT: usize = 384;
const ACTION_CAP: usize = 64;
const ACTION_TIMEOUT: Duration = Duration::from_secs(10);
// The bundled 26.1.2 packets report assigns CLIENTBOUND_CHUNK_BATCH_FINISHED 0x0b.
// mc-protocol exposes its serverbound response, but not this clientbound packet.
const CHUNK_BATCH_FINISHED: i32 = 0x0b;

#[derive(Debug, Clone, Parser)]
#[command(about = "Bounded-memory offline 26.1.2 load players (JSON stdout)")]
struct Args {
    #[arg(long)]
    addr: SocketAddr,
    #[arg(long, value_parser = clap::value_parser!(u32).range(1..=4096))]
    clients: u32,
    #[arg(long, value_parser = clap::value_parser!(u8).range(1..=32))]
    view_distance: u8,
    #[arg(long, default_value_t = 256, value_parser = clap::value_parser!(u32).range(256..=1000000))]
    spacing: u32,
    #[arg(long, default_value_t = 1, value_parser = clap::value_parser!(u32).range(1..=64))]
    group_size: u32,
    #[arg(long)]
    report: PathBuf,
    #[arg(long, help = "JSON array of dry terrain anchor [x,z] coordinates")]
    anchors: Option<PathBuf>,
    #[arg(skip)]
    anchor_positions: Option<Arc<Vec<(i32, i32)>>>,
    #[arg(long, conflicts_with_all = ["explore", "anchor_chunk_walk", "unassisted_login"],
        help = "JSON cardinal street waypoint routes, one closed route per client")]
    street_routes: Option<PathBuf>,
    #[arg(skip)]
    street_paths: Arc<StreetRoutes>,
    #[arg(
        long,
        help = "JSON setup and recurring ordinary commands for load fixtures"
    )]
    commands_file: Option<PathBuf>,
    #[arg(skip)]
    commands: Arc<CommandSchedule>,
    #[arg(
        long,
        help = "Keep the loaded fixture resident without unrelated block edits"
    )]
    no_edits: bool,
    #[arg(long, help = "Walk straight instead of turning every 200 ticks")]
    explore: bool,
    #[arg(
        long,
        conflicts_with = "explore",
        help = "Keep walking and safe landing inside the initial anchor chunk"
    )]
    anchor_chunk_walk: bool,
    #[arg(long, help = "Perform ordinary client respawn after survival death")]
    respawn: bool,
    #[arg(
        long,
        help = "Switch each player to survival after its safe creative landing"
    )]
    survival: bool,
    #[arg(
        long,
        help = "Record bounded per-action block-edit diagnostics in the final report"
    )]
    action_log: bool,
    #[arg(long, value_delimiter = ',', requires = "wire_trace_file")]
    wire_trace_bots: Vec<u32>,
    #[arg(long, requires = "wire_trace_bots")]
    wire_trace_file: Option<PathBuf>,
    #[arg(skip)]
    wire_trace_sink: Option<Arc<Mutex<std::io::BufWriter<std::fs::File>>>>,
    #[arg(
        long,
        default_value_t = 300,
        help = "Per-bot target-window readiness deadline in seconds"
    )]
    prepare_seconds: u64,
    #[arg(
        long,
        default_value_t = 3,
        value_parser = clap::value_parser!(u32).range(1..=4096),
        help = "Maximum client preparations (login through landing) in flight at once"
    )]
    prepare_concurrency: u32,
    #[arg(
        long,
        requires = "schedule_file",
        help = "Use fixed wall-time demand, waiting for stdin run"
    )]
    open_loop: bool,
    #[arg(
        long,
        requires = "open_loop",
        help = "Absolute strict JSON open-loop windows file"
    )]
    schedule_file: Option<PathBuf>,
    #[arg(skip)]
    schedule: Arc<OpenLoopSchedule>,
    #[arg(skip)]
    epoch: Arc<Mutex<Option<RunEpoch>>>,
    #[arg(
        long,
        help = "Login concurrently at the real survival spawn without creative, teleport or inventory assistance"
    )]
    unassisted_login: bool,
    #[arg(
        long,
        help = "Soak only: a bot whose connection drops logs in again instead of ending the run"
    )]
    reconnect: bool,
    #[arg(skip)]
    ended: Arc<Mutex<Option<Duration>>>,
    #[arg(
        long,
        requires = "unassisted_login",
        help = "Verify decoded fresh spawn platform center and four corners"
    )]
    verify_platform: bool,
    #[arg(
        long,
        help = "Bounded received-state mechanic probes and finite public actions JSON"
    )]
    mechanic_spec: Option<PathBuf>,
    #[arg(
        long,
        requires = "mechanic_spec",
        help = "Preserve login state; fresh cell/entity and assigned menu readbacks only"
    )]
    mechanic_read_only: bool,
    #[arg(skip)]
    mechanics: Option<Arc<MechanicMutex<Mechanics>>>,
    #[arg(
        long,
        conflicts_with_all = ["survival", "unassisted_login", "mechanic_read_only"],
        requires = "open_loop",
        help = "Transition each prepared client to survival once at the common Run epoch"
    )]
    survival_on_run: bool,
    #[arg(
        long,
        requires = "mechanic_spec",
        help = "Read-only native fixture snapshot; do not connect to a server"
    )]
    native_snapshot: Option<PathBuf>,
    #[arg(
        long,
        default_value = "scale",
        help = "Offline username prefix (1..=11 ASCII letters, digits or underscores)"
    )]
    name_prefix: String,
    #[arg(
        long,
        value_parser = auth_password_parser,
        help = "Authenticate each bot with solaris-auth (/register, else /login) right after entering Play (4-128 bytes, no whitespace)"
    )]
    auth_password: Option<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandSchedule {
    #[serde(default)]
    setup: Vec<String>,
    #[serde(default)]
    recurring: Vec<RecurringCommands>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecurringCommands {
    every_ticks: u64,
    commands: Vec<String>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct OpenLoopSchedule {
    windows: Vec<LoadWindow>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LoadWindow {
    name: String,
    duration_ms: u64,
    #[serde(default)]
    movement_hz: u32,
    #[serde(default)]
    edit_hz: u32,
    #[serde(default)]
    command_hz: u32,
    #[serde(default)]
    command_interval_ms: Option<u64>,
    #[serde(default)]
    chat_hz: u32,
    #[serde(default)]
    commands: Vec<String>,
    #[serde(default)]
    recovery: bool,
}

impl OpenLoopSchedule {
    fn validate(&self) -> Result<()> {
        ensure!(
            !self.windows.is_empty() && self.windows.len() <= 128,
            "schedule needs 1..=128 windows"
        );
        let mut names = HashSet::new();
        let mut total = 0u64;
        for window in &self.windows {
            ensure!(
                !window.name.is_empty() && names.insert(&window.name),
                "window names must be nonempty and unique"
            );
            ensure!(
                window.duration_ms > 0,
                "window duration_ms must be positive"
            );
            total = total
                .checked_add(window.duration_ms)
                .context("schedule duration overflow")?;
            ensure!(total <= 7 * 24 * 3600 * 1000, "schedule exceeds seven days");
            ensure!(
                window.rates().iter().all(|rate| *rate <= 1000),
                "rates must be <=1000 Hz"
            );
            ensure!(
                window.commands.len() <= 256
                    && window.commands.iter().all(|command| !command.is_empty()
                        && command.len() <= 4096
                        && !command.contains(['\n', '\r'])),
                "window commands must be bounded nonempty single lines"
            );
            ensure!(
                window
                    .command_interval_ms
                    .is_none_or(|ms| ms > 0 && ms <= 7 * 24 * 3600 * 1000)
                    && !(window.command_hz > 0 && window.command_interval_ms.is_some()),
                "command_interval_ms must be positive and mutually exclusive with command_hz"
            );
            ensure!(
                (window.command_hz == 0 && window.command_interval_ms.is_none())
                    || !window.commands.is_empty(),
                "command rate/interval requires commands"
            );
            ensure!(
                !window.recovery
                    || (window.rates() == [0; 4]
                        && window.command_interval_ms.is_none()
                        && window.commands.is_empty()),
                "recovery forbids gameplay rates and commands"
            );
        }
        Ok(())
    }

    fn duration(&self) -> Duration {
        Duration::from_millis(self.windows.iter().map(|window| window.duration_ms).sum())
    }

    fn window_at(&self, elapsed: Duration) -> Option<usize> {
        let mut end = Duration::ZERO;
        for (index, window) in self.windows.iter().enumerate() {
            end += Duration::from_millis(window.duration_ms);
            if elapsed < end {
                return Some(index);
            }
        }
        None
    }
}

impl LoadWindow {
    fn rates(&self) -> [u32; 4] {
        [
            self.movement_hz,
            self.edit_hz,
            self.command_hz,
            self.chat_hz,
        ]
    }
    fn command_slots(&self, elapsed: Duration) -> u64 {
        self.command_interval_ms.map_or_else(
            || elapsed_slots(elapsed, self.command_hz),
            |ms| (elapsed.as_millis() / u128::from(ms)) as u64,
        )
    }
    fn due_command_slot(&self, elapsed: Duration, accounted: &mut u64) -> Option<(u64, u64)> {
        let Some(ms) = self.command_interval_ms else {
            return due_slot(elapsed, self.command_hz, accounted);
        };
        let due = self.command_slots(elapsed);
        if due <= *accounted {
            return None;
        }
        *accounted = due;
        Some((
            due,
            ((elapsed.as_nanos() - u128::from(due) * u128::from(ms) * 1_000_000) / 1000) as u64,
        ))
    }
}

#[derive(Debug, Clone, Copy)]
struct RunEpoch {
    instant: Instant,
    unix_ms: u64,
    unix_ns: u128,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case", deny_unknown_fields)]
enum RunControl {
    Run {},
    Stop {},
}

#[derive(Default, Clone, Serialize)]
struct SlotCounters {
    admitted: u64,
    writes_completed: u64,
    completed: u64,
    dropped_capacity: u64,
    dropped_unavailable: u64,
    drift_samples: u64,
    drift_total_us: u64,
    drift_max_us: u64,
}

impl SlotCounters {
    fn merge(&mut self, other: &Self) {
        self.admitted += other.admitted;
        self.writes_completed += other.writes_completed;
        self.completed += other.completed;
        self.dropped_capacity += other.dropped_capacity;
        self.dropped_unavailable += other.dropped_unavailable;
        self.drift_samples += other.drift_samples;
        self.drift_total_us += other.drift_total_us;
        self.drift_max_us = self.drift_max_us.max(other.drift_max_us);
    }

    fn json(&self, offered: u64) -> Value {
        let mut value = serde_json::to_value(self).expect("slot counters serialize");
        value["offered"] = json!(offered);
        value["dropped_late"] = json!(
            offered
                .saturating_sub(self.admitted + self.dropped_capacity + self.dropped_unavailable)
        );
        value["dropped"] = json!(offered.saturating_sub(self.admitted));
        value
    }
}

#[derive(Default, Clone, Serialize)]
struct WorkReceipts {
    chunks_received: u64,
    breaks_acked: u64,
    places_acked: u64,
    break_updates_observed: u64,
    place_updates_observed: u64,
    explosions_received: u64,
    projectiles_spawned: u64,
    projectiles_observed: u64,
    command_feedback_received: u64,
    disconnects: u64,
    chat_echoes_received: u64,
}

impl WorkReceipts {
    fn from_counters(counters: &Counters) -> Self {
        Self {
            chunks_received: counters.chunks_received,
            breaks_acked: counters.breaks_acked,
            places_acked: counters.places_acked,
            break_updates_observed: counters.break_updates_observed,
            place_updates_observed: counters.place_updates_observed,
            explosions_received: counters.explosions_received,
            projectiles_spawned: counters.projectiles_spawned,
            projectiles_observed: counters.projectiles_observed,
            command_feedback_received: counters.command_feedback_received,
            disconnects: counters.disconnects,
            chat_echoes_received: counters.chat_echoes_received,
        }
    }

    fn add_delta(&mut self, after: &Self, before: &Self) {
        macro_rules! delta { ($($field:ident),*) => { $(self.$field += after.$field - before.$field;)* }; }
        delta!(
            chunks_received,
            breaks_acked,
            places_acked,
            break_updates_observed,
            place_updates_observed,
            explosions_received,
            projectiles_spawned,
            projectiles_observed,
            command_feedback_received,
            disconnects,
            chat_echoes_received
        );
    }
}

#[derive(Default, Clone, Serialize)]
struct WindowCounters {
    movement: SlotCounters,
    edit: SlotCounters,
    command: SlotCounters,
    chat: SlotCounters,
    received: WorkReceipts,
    automatic_respawn_requests: u64,
    automatic_respawn_deferred: u64,
}

impl WindowCounters {
    fn slots_mut(&mut self, kind: usize) -> &mut SlotCounters {
        match kind {
            0 => &mut self.movement,
            1 => &mut self.edit,
            2 => &mut self.command,
            _ => &mut self.chat,
        }
    }

    fn merge(&mut self, other: &Self) {
        self.movement.merge(&other.movement);
        self.edit.merge(&other.edit);
        self.command.merge(&other.command);
        self.chat.merge(&other.chat);
        self.automatic_respawn_requests += other.automatic_respawn_requests;
        self.automatic_respawn_deferred += other.automatic_respawn_deferred;
        self.received
            .add_delta(&other.received, &WorkReceipts::default());
    }
}

// Slots occur at 1/rate, 2/rate, ... including the terminal boundary.
// Integer nanosecond arithmetic avoids accumulated interval/float drift.
fn elapsed_slots(elapsed: Duration, rate: u32) -> u64 {
    ((elapsed.as_nanos() * u128::from(rate)) / 1_000_000_000) as u64
}

fn due_slot(elapsed: Duration, rate: u32, accounted: &mut u64) -> Option<(u64, u64)> {
    let due = elapsed_slots(elapsed, rate);
    if due <= *accounted {
        return None;
    }
    *accounted = due;
    let deadline_ns = (u128::from(due) * 1_000_000_000).div_ceil(u128::from(rate));
    Some((due, ((elapsed.as_nanos() - deadline_ns) / 1000) as u64))
}

#[derive(Clone)]
struct Histogram {
    buckets: [u64; 64],
    samples: u64,
    max_us: u64,
}
impl Default for Histogram {
    fn default() -> Self {
        Self {
            buckets: [0; 64],
            samples: 0,
            max_us: 0,
        }
    }
}
impl Histogram {
    fn record(&mut self, elapsed: Duration) {
        let us = elapsed.as_micros().min(u128::from(u64::MAX)) as u64;
        let bucket = if us <= 1 {
            0
        } else {
            (64 - (us - 1).leading_zeros()).min(63) as usize
        };
        self.buckets[bucket] += 1;
        self.samples += 1;
        self.max_us = self.max_us.max(us);
    }
    fn merge(&mut self, other: &Self) {
        for (a, b) in self.buckets.iter_mut().zip(other.buckets) {
            *a += b;
        }
        self.samples += other.samples;
        self.max_us = self.max_us.max(other.max_us);
    }
    fn percentile(&self, percent: u64) -> u64 {
        if self.samples == 0 {
            return 0;
        }
        let rank = (self.samples * percent).div_ceil(100);
        let mut total = 0;
        for (index, count) in self.buckets.iter().enumerate() {
            total += count;
            if total >= rank {
                return (1u64 << index).min(self.max_us);
            }
        }
        self.max_us
    }
    fn json(&self) -> Value {
        json!({"samples": self.samples, "p50_us": self.percentile(50),
            "p95_us": self.percentile(95), "p99_us": self.percentile(99),
            "max_us": self.max_us, "histogram": self.buckets.as_slice(),
            "histogram_bucket_upper_us": "2^index; final bucket includes overflow"})
    }
}

#[derive(Default, Clone, Serialize)]
struct Counters {
    packets_received: u64,
    decoded_bytes_received: u64,
    chunks_received: u64,
    movements_sent: u64,
    walking_distance_blocks: f64,
    street_waypoints_reached: u64,
    chunk_boundaries_crossed: u64,
    route_refusals_missing_column: u64,
    route_refusals_not_walkable: u64,
    route_refusals_height_delta: u64,
    breaks_sent: u64,
    places_sent: u64,
    block_acks: u64,
    block_updates: u64,
    breaks_acked: u64,
    places_acked: u64,
    break_updates_observed: u64,
    place_updates_observed: u64,
    keepalives_replied: u64,
    chunk_batches_acked: u64,
    teleports_confirmed: u64,
    sprint_toggles: u64,
    chats_sent: u64,
    commands_sent: u64,
    command_feedback_received: u64,
    fixture_input_fences_sent: u64,
    fixture_input_fences_completed: u64,
    respawn_requests: u64,
    respawn_deferred: u64,
    respawns_received: u64,
    explosions_received: u64,
    projectiles_spawned: u64,
    projectiles_observed: u64,
    entity_moves_received: u64,
    health_damage_received: u64,
    action_ack_timeouts: u64,
    action_observation_timeouts: u64,
    disconnects: u64,
    chat_echoes_received: u64,
    fixture_platform_checks: u64,
    #[serde(skip)]
    windows: Vec<WindowCounters>,
    #[serde(skip)]
    latency: Histogram,
    #[serde(skip)]
    actions: Vec<ActionRecord>,
}
impl Counters {
    fn merge(&mut self, other: &Self) {
        macro_rules! sum { ($($field:ident),*) => { $(self.$field += other.$field;)* }; }
        sum!(
            packets_received,
            decoded_bytes_received,
            chunks_received,
            movements_sent,
            street_waypoints_reached,
            breaks_sent,
            places_sent,
            block_acks,
            block_updates,
            breaks_acked,
            places_acked,
            break_updates_observed,
            place_updates_observed,
            keepalives_replied,
            chunk_batches_acked,
            teleports_confirmed,
            sprint_toggles,
            chats_sent,
            commands_sent,
            command_feedback_received,
            fixture_input_fences_sent,
            fixture_input_fences_completed,
            respawn_requests,
            respawn_deferred,
            respawns_received,
            explosions_received,
            projectiles_spawned,
            projectiles_observed,
            entity_moves_received,
            health_damage_received,
            route_refusals_missing_column,
            route_refusals_not_walkable,
            route_refusals_height_delta,
            action_ack_timeouts,
            action_observation_timeouts,
            disconnects
        );
        self.chat_echoes_received += other.chat_echoes_received;
        self.fixture_platform_checks += other.fixture_platform_checks;
        self.walking_distance_blocks += other.walking_distance_blocks;
        self.chunk_boundaries_crossed += other.chunk_boundaries_crossed;
        self.latency.merge(&other.latency);
        if self.windows.len() < other.windows.len() {
            self.windows
                .resize(other.windows.len(), WindowCounters::default());
        }
        for (window, other) in self.windows.iter_mut().zip(&other.windows) {
            window.merge(other);
        }
        self.actions.extend(other.actions.iter().cloned());
    }
}

#[derive(Clone, Copy, Default)]
struct StateFacts {
    passable: bool,
    support: bool,
}
struct Facts {
    states: Vec<StateFacts>,
    dirt_item: u32,
    dirt_state: i32,
    air_state: i32,
    projectile_type_ids: [i32; 2],
    grass_states: Vec<i32>,
}
impl Facts {
    fn load() -> Result<Self> {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla/reports");
        let blocks = mc_data::blocks::load_blocks_report(root.join("blocks.json"))?;
        let max_id = blocks
            .iter()
            .flat_map(|b| b.states.iter().map(|s| s.id))
            .max()
            .context("empty block registry")? as usize;
        let mut states = vec![StateFacts::default(); max_id + 1];
        let mut dirt_state = None;
        let mut air_state = None;
        let mut grass_states = Vec::new();
        for block in blocks {
            let name = block.id.path();
            // Deliberately only walk/edit on known full-cube terrain. Fluids, leaves,
            // fences and partial shapes cannot masquerade as ground in this driver.
            let support = matches!(
                name,
                "stone"
                    | "dirt"
                    | "grass_block"
                    | "sand"
                    | "red_sand"
                    | "gravel"
                    | "deepslate"
                    | "granite"
                    | "diorite"
                    | "andesite"
                    | "tuff"
                    | "calcite"
                    | "snow_block"
                    | "podzol"
                    | "mycelium"
                    | "coarse_dirt"
                    | "rooted_dirt"
                    | "clay"
                    | "terracotta"
                    | "cobblestone"
                    | "moss_block"
                    | "bedrock"
                    | "stone_bricks"
                    | "mossy_stone_bricks"
                    | "cracked_stone_bricks"
                    | "chiseled_stone_bricks"
                    | "mossy_cobblestone"
                    | "bricks"
                    | "sandstone"
                    | "smooth_sandstone"
                    | "cut_sandstone"
                    | "chiseled_sandstone"
                    | "red_sandstone"
                    | "smooth_red_sandstone"
                    | "cut_red_sandstone"
                    | "chiseled_red_sandstone"
                    | "smooth_stone"
                    | "polished_andesite"
                    | "polished_diorite"
                    | "polished_granite"
                    | "quartz_block"
                    | "smooth_quartz"
                    | "quartz_bricks"
                    | "chiseled_quartz_block"
            ) || name.ends_with("_planks")
                || name.ends_with("_concrete")
                || name.ends_with("_terracotta")
                || name.ends_with("_wool");
            let passable = matches!(
                name,
                "air"
                    | "cave_air"
                    | "void_air"
                    | "short_grass"
                    | "tall_grass"
                    | "fern"
                    | "large_fern"
                    | "dandelion"
                    | "poppy"
                    | "dead_bush"
            );
            for state in block.states {
                states[state.id as usize] = StateFacts { passable, support };
                if name == "dirt" && state.default {
                    dirt_state = Some(state.id as i32);
                }
                if name == "air" && state.default {
                    air_state = Some(state.id as i32);
                }
                if name == "grass_block" {
                    grass_states.push(state.id as i32);
                }
            }
        }
        let dirt_item = mc_data::items::load_items_report(root.join("registries.json"))?
            .into_iter()
            .find(|item| item.id.path() == "dirt")
            .context("dirt missing from item registry")?
            .protocol_id;
        let mut projectile_type_ids = [0; 2];
        for (id, name) in projectile_type_ids
            .iter_mut()
            .zip(["minecraft:arrow", "minecraft:snowball"])
        {
            let contract = mc_data::entity_types::entity_type_contract_26_1_2_by_name(name)
                .context("projectile missing from canonical entity registry")?;
            *id = i32::try_from(contract.protocol_id)?;
        }
        Ok(Self {
            states,
            dirt_item,
            dirt_state: dirt_state.context("dirt block state missing")?,
            air_state: air_state.context("air block state missing")?,
            projectile_type_ids,
            grass_states,
        })
    }
    fn get(&self, state: i32) -> StateFacts {
        usize::try_from(state)
            .ok()
            .and_then(|id| self.states.get(id))
            .copied()
            .unwrap_or_default()
    }
}

#[derive(Clone, Copy, Default)]
struct Column {
    feet_y: i16,
    top: i32,
    below: i32,
    walkable: bool,
}
struct Surface {
    x: i32,
    z: i32,
    columns: [Column; 256],
    /// Exact wire cells at y=63,64,65 for the five declared fixture probes.
    /// Kept separately from advertised heightmaps; no expanded chunk storage.
    platform_cells: Option<[i32; 3]>,
}

// A borrowed palette/storage view: no expanded sections, block maps, or chunk body copies.
struct Palette<'a> {
    bits: usize,
    values: [i32; 256],
    len: usize,
    words: &'a [u8],
}
impl<'a> Palette<'a> {
    fn read(data: &mut &'a [u8], entries: usize, direct_threshold: usize) -> Result<Self> {
        let bits = data.read_u8()? as usize;
        ensure!(bits <= 32, "invalid palette bit width {bits}");
        let mut values = [0; 256];
        let len = if bits == 0 {
            values[0] = data.read_varint()?;
            1
        } else if bits < direct_threshold {
            let len = usize::try_from(data.read_varint()?)?;
            ensure!(
                len > 0 && len <= 256 && len <= 1usize << bits,
                "invalid palette size"
            );
            for value in &mut values[..len] {
                *value = data.read_varint()?;
            }
            len
        } else {
            0
        };
        let bytes = 64usize
            .checked_div(bits)
            .map_or(0, |per_word| entries.div_ceil(per_word) * 8);
        ensure!(data.len() >= bytes, "truncated palette storage");
        let (words, rest) = data.split_at(bytes);
        *data = rest;
        Ok(Self {
            bits,
            values,
            len,
            words,
        })
    }
    fn get(&self, index: usize) -> Result<i32> {
        if self.bits == 0 {
            return Ok(self.values[0]);
        }
        let per_word = 64 / self.bits;
        let offset = index / per_word * 8;
        let word = u64::from_be_bytes(self.words[offset..offset + 8].try_into()?);
        let value = ((word >> (index % per_word * self.bits)) & ((1u64 << self.bits) - 1)) as usize;
        if self.len == 0 {
            Ok(value as i32)
        } else {
            ensure!(value < self.len, "palette index outside palette");
            Ok(self.values[value])
        }
    }
}

fn observed_column(
    sections: &[Option<Palette<'_>>; HEIGHT / 16],
    index: usize,
    facts: &Facts,
) -> Result<Column> {
    let (mut above_one, mut above_two) = (true, true);
    // A support block lacking two passable cells above it (a trunk, leaves,
    // water, a flower the model does not know) means the surface is obstructed.
    // A deeper cave floor found below it is not reachable by walking, and must
    // not be reported as terrain the body can enter.
    let mut obstructed = false;
    for section in (0..sections.len()).rev() {
        let blocks = sections[section].as_ref().expect("decoded section");
        if blocks.bits == 0 {
            let state = facts.get(blocks.values[0]);
            if !state.support || !(state.passable || above_one && above_two) {
                obstructed |= state.support;
                above_one = state.passable;
                above_two = state.passable;
                continue;
            }
        }
        for local_y in (0..16).rev() {
            let state_id = blocks.get(local_y * 256 + index)?;
            let state = facts.get(state_id);
            if state.support && above_one && above_two {
                let below = if local_y > 0 {
                    blocks.get((local_y - 1) * 256 + index)?
                } else if section > 0 {
                    sections[section - 1]
                        .as_ref()
                        .expect("decoded lower section")
                        .get(15 * 256 + index)?
                } else {
                    -1
                };
                return Ok(Column {
                    feet_y: (MIN_Y + section as i32 * 16 + local_y as i32 + 1) as i16,
                    top: state_id,
                    below,
                    walkable: !obstructed,
                });
            }
            obstructed |= state.support;
            above_two = above_one;
            above_one = state.passable;
        }
    }
    Ok(Column::default())
}

#[cfg(test)]
fn read_surface(body: &[u8], facts: &Facts) -> Result<Surface> {
    read_surface_mechanics(body, facts, None)
}

type MechanicProbe<'a> = (&'a Arc<MechanicMutex<Mechanics>>, u32, Option<&'a str>);

fn read_surface_mechanics(
    mut body: &[u8],
    facts: &Facts,
    probes: Option<MechanicProbe<'_>>,
) -> Result<Surface> {
    let x = body.read_i32()?;
    let z = body.read_i32()?;
    let probes = probes.filter(|(shared, bot, _)| {
        let state = shared.lock();
        state.chunks.get(&(x, z)).is_some_and(|indices| {
            indices.iter().any(|index| {
                state.cells[*index].observer_bot.is_none()
                    || state.cells[*index].observer_bot == Some(*bot)
            })
        })
    });
    let count = usize::try_from(body.read_varint()?)?;
    ensure!(count <= 16, "too many heightmaps");
    // Command-authored terrain can invalidate advertised heightmaps. Decode
    // the framing, but derive support and headroom from the received sections.
    for _ in 0..count {
        let _kind = body.read_varint()?;
        let len = usize::try_from(body.read_varint()?)?;
        ensure!(
            len <= 256 && body.len() >= len * 8,
            "invalid heightmap storage"
        );
        body = &body[len * 8..];
    }
    let len = usize::try_from(body.read_varint()?)?;
    ensure!(len <= body.len(), "truncated chunk section data");
    let mut data = &body[..len];
    let mut sections: [Option<Palette<'_>>; HEIGHT / 16] = std::array::from_fn(|_| None);
    let mut platform_cells =
        matches!((x, z), (0, 0) | (-2, -2) | (-2, 2) | (2, -2) | (2, 2)).then_some([-1; 3]);
    for (section, slot) in sections.iter_mut().enumerate() {
        let _non_air = data.read_i16()?;
        let _fluid = data.read_i16()?;
        let blocks = Palette::read(&mut data, 4096, 9)?;
        let _biomes = Palette::read(&mut data, 64, 4)?;
        let bottom = MIN_Y + section as i32 * 16;
        if let Some((shared, bot, window)) = probes {
            let mut state = shared.lock();
            let count = state.chunks.get(&(x, z)).map_or(0, Vec::len);
            for offset in 0..count {
                let index = state.chunks[&(x, z)][offset];
                let [px, py, pz] = state.spec.blocks[index].position;
                if (bottom..bottom + 16).contains(&py) {
                    let cell = (py - bottom) as usize * 256
                        + pz.rem_euclid(16) as usize * 16
                        + px.rem_euclid(16) as usize;
                    state.cell(bot, index, blocks.get(cell)?, true, window);
                }
            }
        }
        if let Some(cells) = platform_cells.as_mut() {
            for (cell, y) in cells.iter_mut().zip(63..=65) {
                if (bottom..bottom + 16).contains(&y) {
                    *cell = blocks.get((y - bottom) as usize * 256)?;
                }
            }
        }
        *slot = Some(blocks);
    }
    ensure!(
        data.is_empty(),
        "chunk geometry is not vanilla overworld (-64..320)"
    );
    let mut columns = [Column::default(); 256];
    for (index, column) in columns.iter_mut().enumerate() {
        *column = observed_column(&sections, index, facts)?;
    }
    Ok(Surface {
        x,
        z,
        columns,
        platform_cells,
    })
}

#[derive(Clone, Copy, PartialEq)]
enum Edit {
    Break,
    Place,
}
struct PendingAck {
    sequence: i32,
    sent: Instant,
    edit: Edit,
}
struct Observation {
    position: i64,
    sent: Instant,
    edit: Edit,
    window: Option<usize>,
}

struct PendingChat {
    expected: String,
    window: usize,
}

const ACTION_LOG_CAP: usize = 256;
const ACTION_EVENT_CAP: usize = 8;

#[derive(Clone, Serialize)]
struct ActionEvent {
    after_ms: u64,
    kind: &'static str,
    state: i32,
}
#[derive(Clone, Serialize)]
struct ActionRecord {
    bot: u32,
    sequence: i32,
    edit: &'static str,
    x: i32,
    y: i32,
    z: i32,
    sent_unix_ms: u64,
    bot_position: [f64; 3],
    column_feet_y: Option<i16>,
    column_top_state: Option<i32>,
    column_below_state: Option<i32>,
    column_walkable: Option<bool>,
    expected_place_state: Option<i32>,
    ack_after_ms: Option<u64>,
    observed_after_ms: Option<u64>,
    timed_out: bool,
    events: Vec<ActionEvent>,
    #[serde(skip)]
    sent: Instant,
}

/// Match rendered literal text without allocating a flattened component.
/// Translations and other dynamic renderers are not an echo oracle.
fn literal_component_matches(tag: &mc_nbt::Tag, expected: &str) -> bool {
    fn consume(tag: &mc_nbt::Tag, remaining: &mut &str, depth: usize) -> bool {
        use mc_nbt::Tag;
        if depth > 128 {
            return false;
        }
        match tag {
            Tag::String(text) => match remaining.strip_prefix(text.as_str()) {
                Some(rest) => {
                    *remaining = rest;
                    true
                }
                None => false,
            },
            Tag::List(list) => list
                .elements
                .iter()
                .all(|child| consume(child, remaining, depth + 1)),
            Tag::Compound(fields) => {
                if fields.iter().any(|(name, _)| {
                    matches!(
                        name.as_str(),
                        "translate" | "selector" | "score" | "keybind" | "nbt"
                    )
                }) {
                    return false;
                }
                let field = |name| {
                    fields
                        .iter()
                        .find(|(key, _)| key == name)
                        .map(|(_, tag)| tag)
                };
                field("text").is_none_or(|text| consume(text, remaining, depth + 1))
                    && field("extra").is_none_or(|extra| consume(extra, remaining, depth + 1))
            }
            _ => false,
        }
    }
    let mut remaining = expected;
    consume(tag, &mut remaining, 0) && remaining.is_empty()
}

/// Accept only the exact public save-success replies emitted by either core.
fn fixture_save_succeeded(text: &str) -> bool {
    if text == "Saved the game" {
        return true;
    }
    let Some(counts) = text.strip_prefix("Saved ") else {
        return false;
    };
    let Some((players, rest)) = counts.split_once(" players, ") else {
        return false;
    };
    let Some((entities, rest)) = rest.split_once(" entities, ") else {
        return false;
    };
    let Some(chunks) = rest.strip_suffix(" chunks") else {
        return false;
    };
    [players, entities, chunks].into_iter().all(|count| {
        count.as_bytes().first().is_some_and(u8::is_ascii_digit) && count.parse::<usize>().is_ok()
    })
}

struct Bot {
    index: u32,
    username: String,
    facts: Arc<Facts>,
    metrics: Counters,
    snapshot: Arc<Mutex<Counters>>,
    seen_projectiles: Arc<Mutex<HashSet<Uuid>>>,
    last_publish: Instant,
    surfaces: HashMap<(i32, i32), Surface>,
    seen: HashSet<(i32, i32)>,
    center: (i32, i32),
    entity_id: i32,
    x: f64,
    y: f64,
    z: f64,
    yaw: f32,
    pitch: f32,
    desired_x: f64,
    desired_y: f64,
    desired_z: f64,
    target_sync: bool,
    center_sync: bool,
    creative: bool,
    game_mode: i32,
    landed: bool,
    ticks: u64,
    direction: u32,
    sprint: bool,
    velocity_y: f64,
    last_on_ground: bool,
    corrections_logged: u8,
    sequence: i32,
    acks: VecDeque<PendingAck>,
    observations: VecDeque<Observation>,
    hole: Option<(i32, i32, i32)>,
    edit_pause_until: u64,
    commands: Arc<CommandSchedule>,
    no_edits: bool,
    explore: bool,
    walk_chunk: Option<(i32, i32)>,
    street_routes: Arc<StreetRoutes>,
    street_waypoint: usize,
    respawn: bool,
    respawn_pending: bool,
    last_health: f32,
    action_log: bool,
    schedule: Arc<OpenLoopSchedule>,
    epoch: Arc<Mutex<Option<RunEpoch>>>,
    open_loop: bool,
    ended: Arc<Mutex<Option<Duration>>>,
    accounted_slots: Vec<[u64; 4]>,
    pending_chats: VecDeque<PendingChat>,
    unassisted_login: bool,
    mechanics: Option<Arc<MechanicMutex<Mechanics>>>,
    mechanic_bot: BotMechanics,
    mechanic_read_only: bool,
    survival_on_run: bool,
    survival_transition_sent: bool,
    view_distance: i32,
    wire_trace_sink: Option<Arc<Mutex<std::io::BufWriter<std::fs::File>>>>,
    wire_trace_sequence: u64,
    reader_control: Option<watch::Sender<Option<u64>>>,
    reader_relay: Option<tokio::task::JoinHandle<()>>,
    auth: Option<BotAuth>,
}
impl Bot {
    fn new(
        index: u32,
        args: &Args,
        facts: Arc<Facts>,
        snapshot: Arc<Mutex<Counters>>,
        seen_projectiles: Arc<Mutex<HashSet<Uuid>>>,
    ) -> Self {
        let group = index / args.group_size;
        let member = index % args.group_size;
        let member_width = (f64::from(args.group_size).sqrt().ceil() as u32).max(1);
        let (base_x, base_z) = if let Some(anchors) = &args.anchor_positions {
            anchors[group as usize]
        } else {
            let groups = args.clients.div_ceil(args.group_size);
            let width = (f64::from(groups).sqrt().ceil() as u32).max(1);
            (
                1024 + (group % width * args.spacing) as i32,
                1024 + (group / width * args.spacing) as i32,
            )
        };
        let (x, z) = args.street_paths.0.get(index as usize).map_or_else(
            || {
                (
                    f64::from(base_x) + 0.5 + f64::from(member % member_width * 4),
                    f64::from(base_z) + 0.5 + f64::from(member / member_width * 4),
                )
            },
            |route| (f64::from(route[0][0]) + 0.5, f64::from(route[0][2]) + 0.5),
        );
        Self {
            index,
            username: format!("{}_{index:04}", args.name_prefix),
            view_distance: i32::from(args.view_distance),
            facts,
            metrics: Counters {
                windows: vec![WindowCounters::default(); args.schedule.windows.len()],
                ..Counters::default()
            },
            snapshot,
            seen_projectiles,
            last_publish: Instant::now(),
            surfaces: HashMap::new(),
            seen: HashSet::new(),
            center: (
                (x.floor() as i32).div_euclid(16),
                (z.floor() as i32).div_euclid(16),
            ),
            entity_id: 0,
            x: 0.0,
            y: 0.0,
            z: 0.0,
            yaw: 0.0,
            pitch: 0.0,
            desired_x: x,
            desired_y: 100.0,
            desired_z: z,
            target_sync: false,
            center_sync: false,
            creative: false,
            game_mode: -1,
            landed: false,
            ticks: 0,
            direction: index.wrapping_mul(0x9e3779b9) % 4,
            sprint: false,
            velocity_y: 0.0,
            last_on_ground: true,
            corrections_logged: 0,
            sequence: 0,
            acks: VecDeque::with_capacity(ACTION_CAP),
            observations: VecDeque::with_capacity(ACTION_CAP),
            hole: None,
            edit_pause_until: 0,
            commands: Arc::clone(&args.commands),
            no_edits: args.no_edits,
            explore: args.explore,
            walk_chunk: args.anchor_chunk_walk.then_some((
                (x.floor() as i32).div_euclid(16),
                (z.floor() as i32).div_euclid(16),
            )),
            street_routes: Arc::clone(&args.street_paths),
            street_waypoint: 1,
            respawn: args.respawn,
            respawn_pending: false,
            last_health: 20.0,
            action_log: args.action_log,
            schedule: Arc::clone(&args.schedule),
            epoch: Arc::clone(&args.epoch),
            open_loop: args.open_loop,
            ended: Arc::clone(&args.ended),
            accounted_slots: vec![[0; 4]; args.schedule.windows.len()],
            pending_chats: VecDeque::with_capacity(ACTION_CAP),
            unassisted_login: args.unassisted_login,
            mechanics: args.mechanics.as_ref().map(Arc::clone),
            mechanic_bot: BotMechanics {
                assigned: args.mechanics.as_ref().map_or_else(Vec::new, |shared| {
                    shared
                        .lock()
                        .actions
                        .iter()
                        .enumerate()
                        .filter(|(_, action)| action.bot == index)
                        .map(|(index, _)| index)
                        .collect()
                }),
                ..BotMechanics::default()
            },
            mechanic_read_only: args.mechanic_read_only,
            survival_on_run: args.survival_on_run,
            survival_transition_sent: false,
            wire_trace_sink: args
                .wire_trace_bots
                .contains(&index)
                .then(|| args.wire_trace_sink.as_ref().map(Arc::clone))
                .flatten(),
            wire_trace_sequence: 0,
            reader_control: None,
            reader_relay: None,
            auth: args.auth_password.clone().map(BotAuth::new),
        }
    }
    fn current_window(&self) -> Option<usize> {
        if self
            .ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return None;
        }
        let epoch = *self.epoch.lock().unwrap_or_else(|e| e.into_inner());
        self.schedule.window_at(epoch?.instant.elapsed())
    }
    fn verify_platform(&mut self) -> Result<()> {
        self.metrics.fixture_platform_checks = 0;
        for (x, z) in [(0_i32, 0_i32), (-32, -32), (-32, 32), (32, -32), (32, 32)] {
            let surface = self
                .surfaces
                .get(&(x.div_euclid(16), z.div_euclid(16)))
                .with_context(|| format!("platform column ({x},{z}) not streamed"))?;
            let cells = surface
                .platform_cells
                .with_context(|| format!("platform column ({x},{z}) lacks exact wire cells"))?;
            ensure!(
                self.facts.grass_states.contains(&cells[0])
                    && cells[1] == self.facts.air_state
                    && cells[2] == self.facts.air_state,
                "platform column ({x},{z}) must have grass at y=63 and air at y=64,65; received states={cells:?} grass_states={:?} air_state={}",
                self.facts.grass_states,
                self.facts.air_state
            );
        }
        self.metrics.fixture_platform_checks = 1;
        Ok(())
    }

    fn gameplay_active(&self) -> bool {
        !self.open_loop
            || self
                .current_window()
                .is_some_and(|index| !self.schedule.windows[index].recovery)
    }
    fn trace_wire(&mut self, kind: &'static str, details: Value) -> Result<()> {
        let Some(sink) = &self.wire_trace_sink else {
            return Ok(());
        };
        let now = Instant::now();
        let epoch = *self.epoch.lock().unwrap_or_else(|e| e.into_inner());
        self.wire_trace_sequence += 1;
        let record = json!({"event":"mechanic_wire_trace", "bot":self.index,
            "sequence":self.wire_trace_sequence, "kind":kind,
            "elapsed_us":epoch.map(|epoch| now.duration_since(epoch.instant).as_micros()),
            "unix_ns":SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos(),
            "details":details});
        let mut sink = sink.lock().unwrap_or_else(|e| e.into_inner());
        serde_json::to_writer(&mut *sink, &record)?;
        sink.write_all(b"\n")?;
        Ok(())
    }
    fn publish(&mut self) {
        *self.snapshot.lock().unwrap_or_else(|e| e.into_inner()) = self.metrics.clone();
        self.last_publish = Instant::now();
    }
    fn column(&self, x: i32, z: i32) -> Option<Column> {
        self.surfaces
            .get(&(x.div_euclid(16), z.div_euclid(16)))
            .map(|surface| surface.columns[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize])
    }
    fn ground_y(&self, x: f64, z: f64) -> Option<i16> {
        // Ordinary standing players occupy a 0.6-wide body, not just the
        // block containing their centre. Detect steps before the body hits them.
        let mut ground = i16::MIN;
        for bx in (x - 0.3).floor() as i32..=(x + 0.3).floor() as i32 {
            for bz in (z - 0.3).floor() as i32..=(z + 0.3).floor() as i32 {
                let column = self.column(bx, bz)?;
                if !column.walkable {
                    return None;
                }
                ground = ground.max(column.feet_y);
            }
        }
        Some(ground)
    }
    fn support_y(&self, x: f64, z: f64, feet_y: f64) -> Option<i16> {
        // Missing or obstructed columns forbid walking, but must not suspend
        // gravity. Only an observed full-cube support at or below the feet can
        // stop a fall; a taller neighbouring column must not hide intact support.
        let mut ground: Option<i16> = None;
        for bx in (x - 0.3).floor() as i32..=(x + 0.3).floor() as i32 {
            for bz in (z - 0.3).floor() as i32..=(z + 0.3).floor() as i32 {
                if let Some(column) = self.column(bx, bz)
                    && f64::from(column.feet_y) <= feet_y + 0.001
                    && self.facts.get(column.top).support
                {
                    ground = Some(ground.map_or(column.feet_y, |y| y.max(column.feet_y)));
                }
            }
        }
        ground
    }
    fn position_sync_reply(
        &mut self,
        packet: &SynchronizePlayerPosition,
    ) -> ServerboundMovePlayerPosRot {
        self.x = packet.x
            + if packet.relative_flags & 1 != 0 {
                self.x
            } else {
                0.0
            };
        self.y = packet.y
            + if packet.relative_flags & 2 != 0 {
                self.y
            } else {
                0.0
            };
        self.z = packet.z
            + if packet.relative_flags & 4 != 0 {
                self.z
            } else {
                0.0
            };
        self.yaw = packet.yaw
            + if packet.relative_flags & 8 != 0 {
                self.yaw
            } else {
                0.0
            };
        self.pitch = packet.pitch
            + if packet.relative_flags & 16 != 0 {
                self.pitch
            } else {
                0.0
            };
        if self.mechanics.is_some() {
            self.mechanic_bot.last_position_sync = Some((Instant::now(), [self.x, self.y, self.z]));
            self.mechanic_bot.pose_sync_expected = false;
            self.mechanic_bot.pose_synced = true;
        }
        self.velocity_y = 0.0;
        self.last_on_ground = false;
        ServerboundMovePlayerPosRot {
            x: self.x,
            y: self.y,
            z: self.z,
            yaw: self.yaw,
            pitch: self.pitch,
            flags: MovePlayerFlags::new(false, false),
        }
    }
    fn window_complete(&self, vd: i32) -> bool {
        self.seen.len() == ((2 * vd + 1) * (2 * vd + 1)) as usize
    }
    async fn teleport(
        &mut self,
        client: &mut Client,
        x: f64,
        y: f64,
        z: f64,
        vd: i32,
    ) -> Result<()> {
        self.desired_x = x;
        self.desired_y = y;
        self.desired_z = z;
        self.center = (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        );
        self.seen.retain(|&(cx, cz)| {
            (cx - self.center.0).abs() <= vd && (cz - self.center.1).abs() <= vd
        });
        self.target_sync = false;
        // The server need not resend an unchanged center packet.
        self.center_sync = self.center
            == (
                (self.x.floor() as i32).div_euclid(16),
                (self.z.floor() as i32).div_euclid(16),
            );
        client
            .write_packet(&ServerboundChatCommand {
                command: format!("tp {x} {y} {z}"),
            })
            .await
    }
    async fn initial_play(&mut self, client: &mut Client, vd: i32) -> Result<()> {
        let mut center_received = false;
        // Cumulative across reconnects: require a teleport of THIS session.
        let teleports_before = self.metrics.teleports_confirmed;
        loop {
            let frame = client.read_frame().await?;
            center_received |= frame.id == SetCenterChunk::ID;
            self.frame(client, frame, vd, true).await?;
            // The initial center and position are independent Play packets.
            // Wait for both, without assuming which one the server sends first.
            if center_received && self.metrics.teleports_confirmed > teleports_before {
                return Ok(());
            }
        }
    }
    async fn prepare(&mut self, args: &Args, admission: Arc<Semaphore>) -> Result<Client> {
        // Keep at most `--prepare-concurrency` complete preparations in flight, including every
        // durable gamemode/tp command through landing and the final mode.
        let _permit = if args.unassisted_login {
            None
        } else {
            Some(admission.acquire_owned().await?)
        };
        let mut client = self.connect_reader(args.addr).await?;
        client.drive_login(args.addr, &self.username).await?;
        client
            .write_packet(
                &mc_protocol::packets::configuration::ServerboundClientInformation {
                    information: information(args.view_distance),
                },
            )
            .await?;
        loader::configure(
            &mut client,
            &args
                .report
                .parent()
                .context("report parent")?
                .join("loader-cache")
                .join(&self.username),
        )
        .await?;
        // Unlike connect_to_play, retain/account every Play frame and dispatch
        // out-of-order keepalives, teleports and chunks rather than discarding them.
        self.initial_play(&mut client, args.view_distance as i32)
            .await?;
        self.authenticate(&mut client, args.view_distance as i32)
            .await?;
        if args.mechanic_read_only {
            self.center = (
                (self.x.floor() as i32).div_euclid(16),
                (self.z.floor() as i32).div_euclid(16),
            );
            let vd = i32::from(args.view_distance);
            self.seen
                .extend(self.surfaces.keys().copied().filter(|&(cx, cz)| {
                    (cx - self.center.0).abs() <= vd && (cz - self.center.1).abs() <= vd
                }));
            while !self.window_complete(vd) {
                let frame = client.read_frame().await?;
                self.frame(&mut client, frame, vd, true).await?;
            }
            self.landed = true;
            self.prepare_mechanic_menus(&mut client, vd).await?;
            self.publish();
            return Ok(client);
        }
        if args.unassisted_login {
            // Retain the real spawn; measured login must not change mode or inventory.
            self.center = (
                (self.x.floor() as i32).div_euclid(16),
                (self.z.floor() as i32).div_euclid(16),
            );
            self.desired_x = self.x;
            self.desired_y = self.y;
            self.desired_z = self.z;
            self.target_sync = true;
            self.walk_chunk = args.anchor_chunk_walk.then_some(self.center);
            let vd = i32::from(args.view_distance);
            self.seen.retain(|&(cx, cz)| {
                (cx - self.center.0).abs() <= vd && (cz - self.center.1).abs() <= vd
            });
            self.seen
                .extend(self.surfaces.keys().copied().filter(|&(cx, cz)| {
                    (cx - self.center.0).abs() <= vd && (cz - self.center.1).abs() <= vd
                }));
            while !self.window_complete(vd) {
                let frame = client.read_frame().await?;
                self.frame(&mut client, frame, vd, true).await?;
            }
            ensure!(
                self.game_mode == 0,
                "unassisted measured login requires a survival spawn"
            );
            if args.verify_platform {
                self.verify_platform()?;
            }
            self.landed = true;
            return Ok(client);
        }
        // Commands must follow the initial teleport ACK: Solaris deliberately
        // rejects /tp while that initial synchronization is still unconfirmed.
        client
            .write_packet(&ServerboundClientInformation {
                information: information(args.view_distance),
            })
            .await?;
        client
            .write_packet(&ServerboundChatCommand {
                command: "gamemode creative".into(),
            })
            .await?;
        self.teleport(
            &mut client,
            self.desired_x,
            self.desired_y,
            self.desired_z,
            args.view_distance as i32,
        )
        .await?;
        loop {
            let frame = client.read_frame().await?;
            self.frame(&mut client, frame, args.view_distance as i32, true)
                .await?;
            if self.creative
                && self.target_sync
                && self.center_sync
                && self.window_complete(args.view_distance as i32)
            {
                if !self.landed {
                    let base_x = self.desired_x.floor() as i32;
                    let base_z = self.desired_z.floor() as i32;
                    // A straight-line explorer needs a runway, not just one
                    // walkable column: score each candidate by how far the bot
                    // can walk straight in its seeded direction before hitting
                    // an unwalkable column, a height step, or the edge of the
                    // streamed window. All group members share the anchor, so
                    // each bot offsets along its own direction to avoid
                    // stacking on the same column.
                    let stride = 4 + i32::try_from(self.index % 4).unwrap_or(0) * 4;
                    let (step_x, step_z) = match self.index.wrapping_mul(0x9e3779b9) % 4 {
                        0 => (1, 0),
                        1 => (0, 1),
                        2 => (-1, 0),
                        _ => (0, -1),
                    };
                    let at = |x: i32, z: i32| -> Option<i16> {
                        let surface = self.surfaces.get(&(x.div_euclid(16), z.div_euclid(16)))?;
                        let column =
                            surface.columns[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize];
                        column.walkable.then_some(column.feet_y)
                    };
                    let mut landing: Option<(i64, i32, i32, i32)> =
                        if let Some(route) = self.street_routes.0.get(self.index as usize) {
                            let y = at(base_x, base_z)
                                .context("street start is not received walkable terrain")?;
                            ensure!(
                                i32::from(y) == route[0][1],
                                "street start height differs from received terrain"
                            );
                            Some((0, base_z, base_x, i32::from(y)))
                        } else {
                            None
                        };
                    for surface in self
                        .surfaces
                        .values()
                        .filter(|_| self.street_routes.0.is_empty())
                    {
                        for (offset, column) in surface.columns.iter().enumerate() {
                            if !column.walkable {
                                continue;
                            }
                            let x = surface.x * 16 + (offset % 16) as i32;
                            let z = surface.z * 16 + (offset / 16) as i32;
                            if self
                                .walk_chunk
                                .is_some_and(|chunk| (x.div_euclid(16), z.div_euclid(16)) != chunk)
                            {
                                continue;
                            }
                            let (sx, sz) = (x + step_x * stride, z + step_z * stride);
                            let start_y = at(sx, sz).unwrap_or(column.feet_y);
                            let mut run: i64 = 0;
                            let (mut px, mut pz) = (sx, sz);
                            while run < 64 {
                                let (qx, qz) = (px + step_x, pz + step_z);
                                match at(qx, qz) {
                                    Some(y) if (i32::from(y) - i32::from(start_y)).abs() <= 1 => {
                                        run += 1;
                                        px = qx;
                                        pz = qz;
                                    }
                                    _ => break,
                                }
                            }
                            let dx = i64::from(sx) - i64::from(base_x);
                            let dz = i64::from(sz) - i64::from(base_z);
                            // Prefer the longest straight runway; break ties
                            // by closeness to the anchor so the window stays
                            // centred on certified terrain.
                            let candidate =
                                (run * 4096 - (dx * dx + dz * dz), sz, sx, i32::from(start_y));
                            if landing.is_none_or(|best| candidate > best) {
                                landing = Some(candidate);
                            }
                        }
                    }
                    let Some((_, z, x, y)) = landing else {
                        // The whole window is water (or otherwise unwalkable):
                        // hop one window further along this bot's direction
                        // and look for solid ground there.
                        let hop = f64::from((2 * args.view_distance as i32 + 1) * 16);
                        let (hop_x, hop_z) = (
                            self.desired_x + f64::from(step_x) * hop,
                            self.desired_z + f64::from(step_z) * hop,
                        );
                        eprintln!(
                            "bot_land_search index={} from=({:.1},{:.1}) to=({hop_x:.1},{hop_z:.1})",
                            self.index, self.desired_x, self.desired_z
                        );
                        let hop_y = self.desired_y;
                        self.teleport(&mut client, hop_x, hop_y, hop_z, args.view_distance as i32)
                            .await?;
                        continue;
                    };
                    self.landed = true;
                    self.teleport(
                        &mut client,
                        x as f64 + 0.5,
                        y as f64,
                        z as f64 + 0.5,
                        args.view_distance as i32,
                    )
                    .await?;
                    continue;
                }
                client
                    .write_packet(&ServerboundSetCreativeModeSlot {
                        slot: 36,
                        item_stack: ItemStack::new(self.facts.dirt_item, 64),
                    })
                    .await?;
                client.write_packet(&ServerboundPlayerLoaded).await?;
                client.write_packet(&ServerboundClientTickEnd).await?;
                if args.survival {
                    client
                        .write_packet(&ServerboundChatCommand {
                            command: "gamemode survival".into(),
                        })
                        .await?;
                    while self.creative {
                        let frame = client.read_frame().await?;
                        self.frame(&mut client, frame, args.view_distance as i32, true)
                            .await?;
                    }
                }
                if args.open_loop {
                    if self.index == 0 {
                        self.setup_commands(&mut client, true).await?;
                    }
                    self.setup_fence(&mut client, "ready", true).await?;
                }
                eprintln!(
                    "bot_landing index={} position=({},{},{}) ground_y={:?} creative={}",
                    self.index,
                    self.x,
                    self.y,
                    self.z,
                    self.ground_y(self.x, self.z),
                    self.creative
                );
                self.seen.clear();
                self.seen.shrink_to_fit();
                self.publish();
                self.prepare_mechanic_menus(&mut client, args.view_distance as i32)
                    .await?;
                return Ok(client);
            }
        }
    }
    async fn frame(
        &mut self,
        client: &mut Client,
        frame: RawFrame,
        vd: i32,
        preparing: bool,
    ) -> Result<()> {
        let receipt_window = (!preparing).then(|| self.current_window()).flatten();
        let before = receipt_window.map(|_| WorkReceipts::from_counters(&self.metrics));
        let mechanic_window = receipt_window.map(|index| self.schedule.windows[index].name.clone());
        self.mechanic_frame(&frame, mechanic_window.as_deref())
            .with_context(|| {
                format!(
                    "mechanic packet=0x{:02x} body_bytes={}",
                    frame.id,
                    frame.body.len()
                )
            })?;
        if frame.id == ClientboundSystemChat::ID {
            self.observe_auth_chat(&frame)?;
        }
        self.metrics.packets_received += 1;
        self.metrics.decoded_bytes_received += frame.body.len() as u64;
        let id = frame.id;
        let mut body = frame.body.as_ref();
        match id {
            LoginPlay::ID => {
                let login = LoginPlay::decode(&mut body)?;
                ensure!(
                    login.dimension_name.to_string() == "minecraft:overworld",
                    "bots require vanilla overworld geometry"
                );
                self.entity_id = login.entity_id;
                self.creative = login.game_mode == 1;
                self.game_mode = i32::from(login.game_mode);
            }
            ClientboundKeepAlive::ID => {
                let packet = ClientboundKeepAlive::decode(&mut body)?;
                client
                    .write_packet(&ServerboundKeepAlive { id: packet.id })
                    .await?;
                self.metrics.keepalives_replied += 1;
            }
            CHUNK_BATCH_FINISHED => {
                let count = body.read_varint()?;
                ensure!(
                    count >= 0 && body.is_empty(),
                    "malformed chunk batch completion"
                );
                client
                    .write_packet(&ServerboundChunkBatchReceived {
                        desired_chunks_per_tick: 8.0,
                    })
                    .await?;
                self.metrics.chunk_batches_acked += 1;
            }
            SynchronizePlayerPosition::ID => {
                let packet = SynchronizePlayerPosition::decode(&mut body)?;
                let previous = (self.x, self.y, self.z);
                let previous_velocity_y = self.velocity_y;
                let position_reply = self.position_sync_reply(&packet);
                if self.wire_trace_sink.is_some() {
                    self.trace_wire(
                        "position_sync_received",
                        json!({
                        "packet_id":SynchronizePlayerPosition::ID,
                        "teleport_id":packet.teleport_id, "relative_flags":packet.relative_flags,
                        "raw_position":[packet.x,packet.y,packet.z],
                        "raw_delta":[packet.dx,packet.dy,packet.dz],
                        "yaw":packet.yaw,"pitch":packet.pitch,
                        "client_before":[previous.0,previous.1,previous.2],
                        "resolved_position":[self.x,self.y,self.z]}),
                    )?;
                }
                if self.metrics.teleports_confirmed == 0 {
                    eprintln!(
                        "bot_initial_spawn index={} position=({},{},{})",
                        self.index, self.x, self.y, self.z
                    );
                } else if !preparing && self.corrections_logged < 3 {
                    self.corrections_logged += 1;
                    eprintln!(
                        "bot_position_sync index={} sample={} tick={} teleport_id={} relative_flags={} client_before_sync={:?} received=({},{},{}) client_ground_y={:?} received_ground_y={:?} last_sent_on_ground={} velocity_y={} creative={} direction={}",
                        self.index,
                        self.corrections_logged,
                        self.ticks,
                        packet.teleport_id,
                        packet.relative_flags,
                        previous,
                        self.x,
                        self.y,
                        self.z,
                        self.ground_y(previous.0, previous.2),
                        self.ground_y(self.x, self.z),
                        self.last_on_ground,
                        previous_velocity_y,
                        self.creative,
                        self.direction
                    );
                }
                if !preparing {
                    // The server's collision rejected this walk: the bot's terrain
                    // model is wrong here. Pushing into the same obstruction
                    // again only repeats the correction (and, hovering on a
                    // step-up the server applied, trips its anti-fly counter).
                    // A real player who is stopped turns instead.
                    self.direction = (self.direction + 1) % 4;
                }
                client
                    .write_packet(&ConfirmTeleportation::accepting(&packet))
                    .await?;
                if self.wire_trace_sink.is_some() {
                    self.trace_wire(
                        "teleport_confirm_written",
                        json!({
                        "packet_id":ConfirmTeleportation::ID,"teleport_id":packet.teleport_id}),
                    )?;
                }
                // Vanilla confirms the teleport and reports its resolved pose.
                // Without the pose report, a later rotation-only packet can
                // carry the edge decoder's pre-teleport position back to core.
                client.write_packet(&position_reply).await?;
                if self.wire_trace_sink.is_some() {
                    self.trace_wire(
                        "teleport_pose_written",
                        json!({
                        "packet_id":ServerboundMovePlayerPosRot::ID,
                        "teleport_id":packet.teleport_id,
                        "position":[position_reply.x,position_reply.y,position_reply.z],
                        "yaw":position_reply.yaw,"pitch":position_reply.pitch,"on_ground":false}),
                    )?;
                }
                self.metrics.teleports_confirmed += 1;
                self.target_sync = (self.x - self.desired_x).abs() < 0.01
                    && (self.y - self.desired_y).abs() < 0.01
                    && (self.z - self.desired_z).abs() < 0.01;
            }
            SetCenterChunk::ID => {
                let packet = SetCenterChunk::decode(&mut body)?;
                if let Some(shared) = &self.mechanics {
                    shared.lock().chunk_center(
                        self.index,
                        [packet.chunk_x, packet.chunk_z],
                        mechanic_window.as_deref(),
                    );
                }
                if preparing && (self.unassisted_login || self.mechanic_read_only) {
                    self.center = (packet.chunk_x, packet.chunk_z);
                }
                if preparing {
                    self.center_sync = (packet.chunk_x, packet.chunk_z) == self.center;
                }
            }
            GameEvent::ID => {
                let event = GameEvent::decode(&mut body)?;
                if event.event == GameEvent::EVENT_CHANGE_GAME_MODE {
                    self.creative = event.value == 1.0;
                    self.game_mode = event.value as i32;
                }
            }
            LevelChunkWithLight::ID => {
                self.metrics.chunks_received += 1;
                let cx = body.read_i32()?;
                let cz = body.read_i32()?;
                self.log_chunk(cx, cz);
                if preparing && (cx - self.center.0).abs() <= vd && (cz - self.center.1).abs() <= vd
                {
                    self.seen.insert((cx, cz));
                }
                let local_center = if preparing {
                    self.center
                } else {
                    (
                        (self.x.floor() as i32).div_euclid(16),
                        (self.z.floor() as i32).div_euclid(16),
                    )
                };
                if (cx - local_center.0).abs() <= vd && (cz - local_center.1).abs() <= vd {
                    // Keep only height/top-support summaries for the current view
                    // neighborhood. Already streamed chunks are not resent when
                    // walking toward them; a 3x3-only cache would stall at its edge.
                    // Never retain packet bodies, section arrays, entities or light.
                    let surface = read_surface_mechanics(
                        frame.body.as_ref(),
                        &self.facts,
                        self.mechanics
                            .as_ref()
                            .map(|shared| (shared, self.index, mechanic_window.as_deref())),
                    )?;
                    self.surfaces.retain(|&(x, z), _| {
                        (x - local_center.0).abs() <= vd && (z - local_center.1).abs() <= vd
                    });
                    self.surfaces.insert((surface.x, surface.z), surface);
                }
            }
            BlockChangedAck::ID => {
                let ack = BlockChangedAck::decode(&mut body)?;
                self.metrics.block_acks += 1;
                // Vanilla acknowledges the highest processed sequence cumulatively.
                while self
                    .acks
                    .front()
                    .is_some_and(|pending| pending.sequence <= ack.sequence)
                {
                    if let Some(pending) = self.acks.pop_front() {
                        self.log_ack(pending.sequence);
                        self.metrics.latency.record(pending.sent.elapsed());
                        match pending.edit {
                            Edit::Break => self.metrics.breaks_acked += 1,
                            Edit::Place => self.metrics.places_acked += 1,
                        }
                    }
                }
            }
            BlockUpdate::ID => {
                let update = BlockUpdate::decode(&mut body)?;
                self.update(
                    update.position,
                    update.state_id,
                    "block_update",
                    mechanic_window.as_deref(),
                );
            }
            SectionBlocksUpdate::ID => {
                let update = SectionBlocksUpdate::decode(&mut body)?;
                let sx = (update.section_pos >> 42) as i32;
                let sy = ((update.section_pos << 44) >> 44) as i32;
                let sz = ((update.section_pos << 22) >> 42) as i32;
                for change in update.changes {
                    let x = sx * 16 + i32::from((change.relative_pos >> 8) & 15);
                    let z = sz * 16 + i32::from((change.relative_pos >> 4) & 15);
                    let y = sy * 16 + i32::from(change.relative_pos & 15);
                    self.update(
                        pack_block_pos(x, y, z),
                        change.state_id,
                        "section_update",
                        mechanic_window.as_deref(),
                    );
                }
            }
            ClientboundExplode::ID => {
                self.metrics.explosions_received += 1;
            }
            AddEntity::ID => {
                let _entity_id = body.read_varint()?;
                let uuid = body.read_uuid()?;
                let entity_type_id = body.read_varint()?;
                if self.facts.projectile_type_ids.contains(&entity_type_id) {
                    self.metrics.projectiles_observed += 1;
                    // One process hosts all clients: dedupe cross-bot so a
                    // widely visible arrow counts one launch, not 16
                    // sightings. Never cleared mid-run: respawn/chunk
                    // re-streams must not recount old arrows.
                    if self
                        .seen_projectiles
                        .lock()
                        .unwrap_or_else(|e| e.into_inner())
                        .insert(uuid)
                    {
                        self.metrics.projectiles_spawned += 1;
                    }
                }
            }
            MoveEntityPos::ID | MoveEntityPosRot::ID | EntityPositionSync::ID => {
                self.metrics.entity_moves_received += 1;
            }
            ClientboundSetHealth::ID => {
                let health = ClientboundSetHealth::decode(&mut body)?;
                let was_dead = self.last_health <= 0.0;
                if !preparing && health.health < self.last_health {
                    self.metrics.health_damage_received += 1;
                }
                self.last_health = health.health;
                if self.respawn
                    && !preparing
                    && !self.mechanic_read_only
                    && health.health <= 0.0
                    && !self.respawn_pending
                {
                    if self.gameplay_active()
                        || (self.reader_control.is_some() && self.current_window().is_some())
                    {
                        let request_window = self.current_window();
                        client
                            .write_packet(&ServerboundClientCommand {
                                action: ClientCommandAction::PerformRespawn,
                            })
                            .await?;
                        self.respawn_pending = true;
                        self.metrics.respawn_requests += 1;
                        if let Some(index) = request_window {
                            self.metrics.windows[index].automatic_respawn_requests += 1;
                        }
                    } else if !was_dead {
                        self.metrics.respawn_deferred += 1;
                        if let Some(index) = receipt_window {
                            self.metrics.windows[index].automatic_respawn_deferred += 1;
                        }
                    }
                }
            }
            ClientboundRespawn::ID if !preparing && (self.respawn || self.mechanics.is_some()) => {
                let respawn = ClientboundRespawn::decode(&mut body)?;
                ensure!(
                    respawn.dimension_name.to_string() == "minecraft:overworld",
                    "load fixture respawn left overworld"
                );
                self.creative = respawn.game_mode == 1;
                self.game_mode = i32::from(respawn.game_mode);
                self.respawn_pending = false;
                self.last_health = 20.0;
                self.metrics.respawns_received += 1;
                client.write_packet(&ServerboundPlayerLoaded).await?;
            }
            ClientboundSystemChat::ID if !preparing => {
                let packet = ClientboundSystemChat::decode(&mut body)?;
                let tag = mc_nbt::read_network(&mut packet.content_nbt.as_slice())?;
                if let Some(index) = self
                    .pending_chats
                    .iter()
                    .position(|pending| literal_component_matches(&tag, &pending.expected))
                {
                    if let Some(pending) = self.pending_chats.remove(index) {
                        self.metrics.windows[pending.window].chat.completed += 1;
                    }
                    self.metrics.chat_echoes_received += 1;
                } else if let mc_nbt::Tag::Compound(fields) = tag {
                    for (name, value) in fields {
                        if name == "text"
                            && let mc_nbt::Tag::String(text) = value
                        {
                            if self.index != 0 {
                                continue;
                            }
                            if !self.commands.setup.is_empty() {
                                emit(
                                    &json!({"event": "fixture_command_feedback", "bot": self.index, "text": text}),
                                )?;
                            }
                            ensure!(
                                !text.starts_with("Unknown command")
                                    && !text.starts_with("You do not have permission")
                                    && !text.starts_with("Unknown entity type")
                                    && !text.starts_with("Cannot summon")
                                    && !text.starts_with("Summon rejected")
                                    && !text.starts_with("Failed to save the game")
                                    && !text.starts_with("Save-all failed")
                                    && !text.starts_with("Usage:"),
                                "fixture command refused: {text}"
                            );
                            self.metrics.command_feedback_received += 1;
                            if fixture_save_succeeded(&text) {
                                emit(
                                    &json!({"event": "fixture_checkpoint_saved", "bot": self.index}),
                                )?;
                            }
                        }
                    }
                }
            }
            PlayDisconnect::ID => {
                let disconnect = PlayDisconnect::decode(&mut body)?;
                bail!(
                    "server disconnect: {}",
                    String::from_utf8_lossy(&disconnect.reason_nbt)
                );
            }
            _ => {}
        }
        if let Some(shared) = &self.mechanics {
            let mut state = shared.lock();
            if state.accepts_receive(mechanic_window.as_deref()) {
                let player = &mut state.players[self.index as usize];
                if let Some(window) = &mechanic_window {
                    let progress = player
                        .reader_progress_by_window
                        .entry(window.clone())
                        .or_default();
                    progress.frames += 1;
                    progress.decoded_bytes += frame.body.len() as u64;
                    progress.position_updates += u64::from(matches!(
                        id,
                        MoveEntityPos::ID
                            | MoveEntityPosRot::ID
                            | EntityPositionSync::ID
                            | SynchronizePlayerPosition::ID
                    ));
                }
            }
        }
        if let (Some(index), Some(before)) = (receipt_window, before) {
            let after = WorkReceipts::from_counters(&self.metrics);
            self.metrics.windows[index]
                .received
                .add_delta(&after, &before);
        }
        self.complete_mechanics(mechanic_window.as_deref());
        self.close_received_menu(client).await?;
        if self.last_publish.elapsed() >= Duration::from_secs(1) {
            self.publish();
        }
        Ok(())
    }
    fn log_action(&mut self, edit: Edit, x: i32, y: i32, z: i32, sent: Instant) {
        if !self.action_log || self.metrics.actions.len() >= ACTION_LOG_CAP {
            return;
        }
        let column = self.column(x, z);
        self.metrics.actions.push(ActionRecord {
            bot: self.index,
            sequence: self.sequence,
            edit: match edit {
                Edit::Break => "break",
                Edit::Place => "place",
            },
            x,
            y,
            z,
            sent_unix_ms: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map_or(0, |elapsed| elapsed.as_millis() as u64),
            bot_position: [self.x, self.y, self.z],
            column_feet_y: column.map(|column| column.feet_y),
            column_top_state: column.map(|column| column.top),
            column_below_state: column.map(|column| column.below),
            column_walkable: column.map(|column| column.walkable),
            expected_place_state: (edit == Edit::Place).then_some(self.facts.dirt_state),
            ack_after_ms: None,
            observed_after_ms: None,
            timed_out: false,
            events: Vec::new(),
            sent,
        });
    }
    fn log_ack(&mut self, sequence: i32) {
        if let Some(record) = self
            .metrics
            .actions
            .iter_mut()
            .rev()
            .find(|record| record.sequence == sequence)
        {
            record.ack_after_ms = Some(record.sent.elapsed().as_millis() as u64);
        }
    }
    fn log_update(&mut self, position: i64, state: i32, via: &'static str) {
        if !self.action_log {
            return;
        }
        let (x, y, z) = unpack_block_pos(position);
        let passable = self.facts.get(state).passable;
        let dirt = self.facts.dirt_state;
        for record in &mut self.metrics.actions {
            if record.observed_after_ms.is_some() || (record.x, record.y, record.z) != (x, y, z) {
                continue;
            }
            let after_ms = record.sent.elapsed().as_millis() as u64;
            if record.events.len() < ACTION_EVENT_CAP {
                record.events.push(ActionEvent {
                    after_ms,
                    kind: via,
                    state,
                });
            }
            let observed = if record.edit == "break" {
                passable
            } else {
                state == dirt
            };
            if observed {
                record.observed_after_ms = Some(after_ms);
            }
        }
    }
    fn log_timeout(&mut self, position: i64) {
        if !self.action_log {
            return;
        }
        let (x, y, z) = unpack_block_pos(position);
        if let Some(record) = self.metrics.actions.iter_mut().find(|record| {
            (record.x, record.y, record.z) == (x, y, z)
                && record.observed_after_ms.is_none()
                && !record.timed_out
        }) {
            record.timed_out = true;
        }
    }
    fn log_chunk(&mut self, cx: i32, cz: i32) {
        if !self.action_log {
            return;
        }
        for record in &mut self.metrics.actions {
            if record.observed_after_ms.is_none()
                && record.x.div_euclid(16) == cx
                && record.z.div_euclid(16) == cz
                && record.events.len() < ACTION_EVENT_CAP
            {
                record.events.push(ActionEvent {
                    after_ms: record.sent.elapsed().as_millis() as u64,
                    kind: "chunk_resend",
                    state: -1,
                });
            }
        }
    }
    fn update(&mut self, position: i64, state: i32, via: &'static str, window: Option<&str>) {
        if let Some(shared) = &self.mechanics {
            shared
                .lock()
                .update_cell(self.index, position, state, window);
        }
        self.log_update(position, state, via);
        self.metrics.block_updates += 1;
        let passable = self.facts.get(state).passable;
        self.observations.retain(|pending| {
            let observed = pending.position == position
                && match pending.edit {
                    Edit::Break => passable,
                    Edit::Place => state == self.facts.dirt_state,
                };
            if observed {
                match pending.edit {
                    Edit::Break => self.metrics.break_updates_observed += 1,
                    Edit::Place => self.metrics.place_updates_observed += 1,
                }
                if let Some(window) = pending.window {
                    self.metrics.windows[window].edit.completed += 1;
                }
            }
            !observed
        });
        let (x, y, z) = unpack_block_pos(position);
        if let Some(surface) = self.surfaces.get_mut(&(x.div_euclid(16), z.div_euclid(16))) {
            let column = &mut surface.columns[(z.rem_euclid(16) * 16 + x.rem_euclid(16)) as usize];
            let feet = i32::from(column.feet_y);
            if y == feet - 1 {
                if passable {
                    column.top = column.below;
                    column.feet_y -= 1;
                    column.below = -1; // A deeper cell was not retained; never invent support.
                    column.walkable = self.facts.get(column.top).support;
                } else {
                    column.top = state;
                    column.walkable = self.facts.get(state).support;
                }
            } else if y == feet {
                if self.facts.get(state).support {
                    column.below = column.top;
                    column.top = state;
                    column.feet_y += 1;
                    column.walkable = true;
                } else if !passable {
                    column.walkable = false;
                }
            } else if y == feet + 1 && !passable {
                column.walkable = false;
            }
        }
        if let Some((hx, hy, hz)) = self.hole
            && (x, y, z) == (hx, hy, hz)
            && state == self.facts.dirt_state
        {
            self.hole = None;
        }
    }
    fn expire_actions(&mut self) {
        while self
            .acks
            .front()
            .is_some_and(|pending| pending.sent.elapsed() >= ACTION_TIMEOUT)
        {
            self.acks.pop_front();
            self.metrics.action_ack_timeouts += 1;
        }
        while self
            .observations
            .front()
            .is_some_and(|pending| pending.sent.elapsed() >= ACTION_TIMEOUT)
        {
            if let Some(expired) = self.observations.pop_front() {
                self.log_timeout(expired.position);
            }
            self.metrics.action_observation_timeouts += 1;
        }
    }
    async fn edit(&mut self, client: &mut Client) -> Result<bool> {
        let window = self.current_window();
        if self.acks.len() >= ACTION_CAP || self.observations.len() >= ACTION_CAP {
            return Ok(false);
        }
        if !self.open_loop && !self.observations.is_empty() {
            return Ok(false);
        }
        let px = self.x.floor() as i32;
        let pz = self.z.floor() as i32;
        let mut candidate = None;
        if let Some((x, y, z)) = self.hole {
            if let Some(column) = self.column(x, z)
                && i32::from(column.feet_y) == y
                && column.walkable
                && !self.mechanic_protected(x, y, z)
                && !self.mechanic_protected(x, y - 1, z)
                && ((x as f64 + 0.5 - self.x).powi(2)
                    + (z as f64 + 0.5 - self.z).powi(2)
                    + (y as f64 - self.y - 1.62).powi(2))
                    < 20.0
            {
                candidate = Some((x, y, z, Edit::Place));
            } else {
                // A route may have left the previous edit behind; do not issue an
                // out-of-reach placement or assume the break succeeded.
                self.hole = None;
            }
        }
        if candidate.is_none() {
            let offset = if self.open_loop {
                (self.sequence as u64 + u64::from(self.index)) % 4
            } else {
                (self.ticks / 80 + u64::from(self.index)) % 4
            };
            for direction in 0..(if self.mechanics.is_some() { 4 } else { 1 }) {
                let (dx, dz) = match (offset + direction) % 4 {
                    0 => (2, 0),
                    1 => (0, 2),
                    2 => (-2, 0),
                    _ => (0, -2),
                };
                let (x, z) = (px + dx, pz + dz);
                if let Some(column) = self.column(x, z)
                    && column.walkable
                    && self.facts.get(column.below).support
                    && (f64::from(column.feet_y) - self.y).abs() <= 1.0
                    && !self.mechanic_protected(x, i32::from(column.feet_y) - 1, z)
                {
                    candidate = Some((x, i32::from(column.feet_y) - 1, z, Edit::Break));
                    break;
                }
            }
        }
        let Some((x, y, z, edit)) = candidate else {
            return Ok(false);
        };
        // Do not count one update as proof for duplicate in-flight requests.
        if self.open_loop
            && self
                .observations
                .iter()
                .any(|pending| pending.position == pack_block_pos(x, y, z))
        {
            return Ok(false);
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .context("block action sequence exhausted")?;
        let sent = Instant::now();
        match edit {
            Edit::Break => {
                client
                    .write_packet(&ServerboundPlayerAction {
                        action: PlayerActionKind::StartDestroyBlock,
                        position: pack_block_pos(x, y, z),
                        direction: Direction::Up,
                        sequence: self.sequence,
                    })
                    .await?;
                self.metrics.breaks_sent += 1;
                self.hole = Some((x, y, z));
                self.edit_pause_until = self.ticks + 20;
            }
            Edit::Place => {
                client
                    .write_packet(&ServerboundUseItemOn {
                        hand: InteractionHand::MainHand,
                        position: pack_block_pos(x, y - 1, z),
                        direction: Direction::Up,
                        cursor_x: 0.5,
                        cursor_y: 1.0,
                        cursor_z: 0.5,
                        inside: false,
                        world_border_hit: false,
                        sequence: self.sequence,
                    })
                    .await?;
                self.metrics.places_sent += 1;
            }
        }
        self.acks.push_back(PendingAck {
            sequence: self.sequence,
            sent,
            edit,
        });
        self.observations.push_back(Observation {
            position: pack_block_pos(x, y, z),
            sent,
            edit,
            window,
        });
        self.log_action(edit, x, y, z, sent);
        Ok(true)
    }
    async fn tick(&mut self, client: &mut Client) -> Result<()> {
        self.tick_demand(client, true).await.map(|_| ())
    }

    async fn tick_demand(&mut self, client: &mut Client, legacy: bool) -> Result<bool> {
        if self.mechanic_read_only {
            client.write_packet(&ServerboundClientTickEnd).await?;
            return Ok(false);
        }
        let demand_window = self.current_window();
        if !legacy && !self.gameplay_active() {
            return Ok(false);
        }
        self.ticks += 1;
        self.expire_actions();
        if legacy && self.index == 0 {
            if self.ticks == 1 {
                self.setup_commands(client, false).await?;
            }
            for recurring in &self.commands.recurring {
                if self.ticks.is_multiple_of(recurring.every_ticks) {
                    for command in &recurring.commands {
                        client
                            .write_packet(&ServerboundChatCommand {
                                command: command.clone(),
                            })
                            .await?;
                        self.metrics.commands_sent += 1;
                    }
                }
            }
        }
        if self.respawn_pending {
            client.write_packet(&ServerboundClientTickEnd).await?;
            return Ok(false);
        }
        // A seeded 200-tick leg is 43-56 blocks, crossing chunk boundaries.
        // Direction changes preserve the accumulated position (no offset teleports).
        if !self.explore && self.ticks.is_multiple_of(200) {
            self.direction = (self.direction + 1) % 4;
        }
        let sprint = (self.ticks / 100 + u64::from(self.index)) % 3 == 1;
        if sprint != self.sprint {
            client
                .write_packet(&ServerboundPlayerCommand {
                    entity_id: self.entity_id,
                    action: if sprint {
                        PlayerCommandAction::StartSprinting
                    } else {
                        PlayerCommandAction::StopSprinting
                    },
                    data: 0,
                })
                .await?;
            self.sprint = sprint;
            self.metrics.sprint_toggles += 1;
        }
        let speed = if self.ticks < self.edit_pause_until {
            0.0
        } else if sprint {
            0.2806
        } else {
            0.21585
        };
        let (dx, dz, yaw) = self
            .street_step(speed)
            .unwrap_or_else(|| match self.direction {
                0 => (speed, 0.0, -90.0),
                1 => (0.0, speed, 0.0),
                2 => (-speed, 0.0, 90.0),
                _ => (0.0, -speed, 180.0),
            });
        let (nx, nz) = (self.x + dx, self.z + dz);
        let mut moving = false;
        let mut jump = false;
        let inside_route = self.walk_chunk.is_none_or(|(cx, cz)| {
            let min_x = f64::from(cx) * 16.0 + 0.5;
            let min_z = f64::from(cz) * 16.0 + 0.5;
            (min_x..=min_x + 15.0).contains(&nx) && (min_z..=min_z + 15.0).contains(&nz)
        });
        if inside_route && let Some(next_ground) = self.ground_y(nx, nz) {
            let ground = f64::from(next_ground);
            let delta = ground - self.y;
            if (-1.01..=1.01).contains(&delta) {
                if delta > 0.0 && self.velocity_y == 0.0 {
                    self.velocity_y = 0.42;
                    jump = true;
                }
                // Ascend before advancing into a full-block step.
                if delta <= 0.0 {
                    let old_chunk = (
                        (self.x.floor() as i32).div_euclid(16),
                        (self.z.floor() as i32).div_euclid(16),
                    );
                    self.metrics.walking_distance_blocks += (dx * dx + dz * dz).sqrt();
                    self.x = nx;
                    self.z = nz;
                    moving = dx != 0.0 || dz != 0.0;
                    let new_chunk = (
                        (self.x.floor() as i32).div_euclid(16),
                        (self.z.floor() as i32).div_euclid(16),
                    );
                    if old_chunk != new_chunk {
                        self.metrics.chunk_boundaries_crossed += 1;
                    }
                } else {
                    self.metrics.route_refusals_height_delta += 1;
                }
            } else if self.velocity_y == 0.0 {
                self.metrics.route_refusals_height_delta += 1;
                self.direction = (self.direction + 1) % 4;
            }
        } else {
            // Distinguish unknown geometry from known-impassable columns so a
            // single-bot causal trace can tell streaming lag from terrain.
            if self.ground_y(nx, nz).is_none() {
                self.metrics.route_refusals_missing_column += 1;
            } else {
                self.metrics.route_refusals_not_walkable += 1;
            }
            self.direction = (self.direction + 1) % 4;
        }
        let ground = self.support_y(self.x, self.z, self.y).map(f64::from);
        let mut on_ground =
            self.velocity_y == 0.0 && ground.is_some_and(|ground| (self.y - ground).abs() < 0.001);
        if !on_ground {
            let previous_y = self.y;
            self.y += self.velocity_y;
            self.velocity_y = (self.velocity_y - 0.08) * 0.98;
            if let Some(ground) = ground
                && previous_y >= ground
                && self.y <= ground
            {
                self.y = ground;
                self.velocity_y = 0.0;
                on_ground = true;
            }
        }
        self.last_on_ground = on_ground;
        if !legacy && (self.current_window() != demand_window || !self.gameplay_active()) {
            return Ok(false);
        }
        client
            .write_packet(&ServerboundPlayerInput {
                input: PlayerInput {
                    forward: moving,
                    jump,
                    sprint,
                    ..PlayerInput::default()
                },
            })
            .await?;
        if !legacy && (self.current_window() != demand_window || !self.gameplay_active()) {
            return Ok(false);
        }
        client
            .write_packet(&ServerboundMovePlayerPosRot {
                x: self.x,
                y: self.y,
                z: self.z,
                yaw,
                pitch: 0.0,
                flags: MovePlayerFlags::new(on_ground, false),
            })
            .await?;
        if self.wire_trace_sink.is_some() {
            self.trace_wire(
                "movement_written",
                json!({
                "packet_id":ServerboundMovePlayerPosRot::ID,
                "position":[self.x,self.y,self.z], "yaw":yaw,"pitch":0.0,
                "on_ground":on_ground,"moving":moving,"jump":jump,"sprint":sprint}),
            )?;
        }
        self.yaw = yaw;
        self.pitch = 0.0;
        self.metrics.movements_sent += 1;
        // Stagger interactions and chat across clients, not a synchronized storm.
        if legacy
            && !self.no_edits
            && self.creative
            && (self.hole.is_some() || (self.ticks + u64::from(self.index) * 17).is_multiple_of(80))
        {
            self.edit(client).await?;
        }
        if legacy && (self.ticks + u64::from(self.index) * 101).is_multiple_of(1200) {
            client
                .write_packet(&ServerboundChat {
                    message: format!("scale {} walking", self.index),
                    timestamp_millis: SystemTime::now()
                        .duration_since(UNIX_EPOCH)?
                        .as_millis()
                        .min(i64::MAX as u128) as i64,
                    salt: i64::from(self.index),
                    signature: None,
                    last_seen_offset: 0,
                    last_seen_acknowledged: [0; 3],
                    last_seen_checksum: 0,
                })
                .await?;
            self.metrics.chats_sent += 1;
        }
        client.write_packet(&ServerboundClientTickEnd).await?;
        if self.last_publish.elapsed() >= Duration::from_secs(1) {
            self.publish();
        }
        Ok(true)
    }
    async fn open_loop_poll(&mut self, client: &mut Client) -> Result<()> {
        let epoch = *self.epoch.lock().unwrap_or_else(|e| e.into_inner());
        let Some(epoch) = epoch else {
            return Ok(());
        };
        if self
            .ended
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .is_some()
        {
            return Ok(());
        }
        self.expire_actions();
        if self.survival_on_run && !self.survival_transition_sent && !self.mechanic_read_only {
            self.survival_transition_sent = true;
            let elapsed = epoch.instant.elapsed();
            if self.schedule.window_at(elapsed) == Some(0) && !self.schedule.windows[0].recovery {
                if let Some(shared) = &self.mechanics {
                    shared.lock().players[self.index as usize].survival_transition_admitted += 1;
                }
                client
                    .write_packet(&ServerboundChatCommand {
                        command: "gamemode survival".into(),
                    })
                    .await?;
                self.metrics.commands_sent += 1;
                if let Some(shared) = &self.mechanics {
                    shared.lock().players[self.index as usize].survival_transition_writes += 1;
                }
            }
        }
        self.mechanic_poll(client, epoch.instant.elapsed()).await?;
        if self.mechanic_read_only {
            return Ok(());
        }
        let schedule = Arc::clone(&self.schedule);
        let mut start = Duration::ZERO;
        for (index, window) in schedule.windows.iter().enumerate() {
            let duration = Duration::from_millis(window.duration_ms);
            let end = start + duration;
            for (kind, rate) in window.rates().into_iter().enumerate() {
                if (rate == 0 && !(kind == 2 && window.command_interval_ms.is_some()))
                    || (kind == 2 && self.index != 0)
                {
                    continue;
                }
                let elapsed = epoch.instant.elapsed();
                let local = elapsed.saturating_sub(start).min(duration);
                let due = if kind == 2 {
                    window.due_command_slot(local, &mut self.accounted_slots[index][kind])
                } else {
                    due_slot(local, rate, &mut self.accounted_slots[index][kind])
                };
                let Some((slot, drift_us)) = due else {
                    continue;
                };
                // Expired windows and all but the newest slot are dropped,
                // never buffered/replayed. Offered counts are independently
                // derived from the common wall epoch in aggregate().
                if elapsed < start || elapsed >= end || window.recovery {
                    continue;
                }
                let counters = self.metrics.windows[index].slots_mut(kind);
                counters.drift_samples += 1;
                counters.drift_total_us += drift_us;
                counters.drift_max_us = counters.drift_max_us.max(drift_us);
                if self.respawn_pending || self.last_health <= 0.0 {
                    counters.dropped_unavailable += 1;
                    continue;
                }
                match kind {
                    0 => {
                        self.metrics.windows[index].movement.admitted += 1;
                        if self.tick_demand(client, false).await? {
                            self.metrics.windows[index].movement.writes_completed += 1;
                        }
                    }
                    1 => {
                        if self.acks.len() >= ACTION_CAP || self.observations.len() >= ACTION_CAP {
                            self.metrics.windows[index].edit.dropped_capacity += 1;
                        } else if self.no_edits || !self.creative {
                            self.metrics.windows[index].edit.dropped_unavailable += 1;
                        } else if self.edit(client).await? {
                            self.metrics.windows[index].edit.admitted += 1;
                            self.metrics.windows[index].edit.writes_completed += 1;
                        } else {
                            self.metrics.windows[index].edit.dropped_unavailable += 1;
                        }
                    }
                    2 => {
                        self.metrics.windows[index].command.admitted += 1;
                        client
                            .write_packet(&ServerboundChatCommand {
                                command: window.commands
                                    [((slot - 1) % window.commands.len() as u64) as usize]
                                    .clone(),
                            })
                            .await?;
                        self.metrics.commands_sent += 1;
                        self.metrics.windows[index].command.writes_completed += 1;
                    }
                    _ => {
                        if self.pending_chats.len() >= ACTION_CAP {
                            self.metrics.windows[index].chat.dropped_capacity += 1;
                            continue;
                        }
                        let message = format!(
                            "scale_open_loop_{}_{}_{}_{}",
                            epoch.unix_ms, self.index, index, slot
                        );
                        self.pending_chats.push_back(PendingChat {
                            expected: format!("<{}> {message}", self.username),
                            window: index,
                        });
                        self.metrics.windows[index].chat.admitted += 1;
                        client
                            .write_packet(&ServerboundChat {
                                message,
                                timestamp_millis: SystemTime::now()
                                    .duration_since(UNIX_EPOCH)?
                                    .as_millis()
                                    .min(i64::MAX as u128)
                                    as i64,
                                salt: i64::from(self.index),
                                signature: None,
                                last_seen_offset: 0,
                                last_seen_acknowledged: [0; 3],
                                last_seen_checksum: 0,
                            })
                            .await?;
                        self.metrics.chats_sent += 1;
                        self.metrics.windows[index].chat.writes_completed += 1;
                    }
                }
            }
            start = end;
        }
        Ok(())
    }
    async fn play(
        &mut self,
        client: &mut Client,
        stop: &mut watch::Receiver<bool>,
        vd: i32,
    ) -> Result<()> {
        let mut ticks = tokio::time::interval(Duration::from_millis(50));
        ticks.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut demand = tokio::time::interval(Duration::from_millis(10));
        demand.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = stop.changed() => return Ok(()),
                _ = demand.tick(), if self.open_loop => self.open_loop_poll(client).await?,
                _ = ticks.tick() => {
                    if self.open_loop {
                        self.expire_actions();
                        client.write_packet(&ServerboundClientTickEnd).await?;
                        if self.last_publish.elapsed() >= Duration::from_secs(1) { self.publish(); }
                    } else {
                        self.tick(client).await?;
                    }
                },
                frame = client.read_frame() => self.frame(client, frame?, vd, false).await?,
            }
        }
    }
}

fn information(view_distance: u8) -> ClientInformation {
    ClientInformation {
        language: "en_us".into(),
        view_distance: view_distance as i8,
        chat_visibility: ChatVisibility::Full,
        chat_colors: true,
        model_customisation: 127,
        main_hand: MainHand::Right,
        text_filtering_enabled: false,
        allows_listing: true,
        particle_status: ParticleStatus::All,
    }
}

fn aggregate(
    snapshots: &[Arc<Mutex<Counters>>],
    args: &Args,
    ready: u32,
    started: Instant,
    event: &str,
    error: Option<&str>,
) -> Result<Value> {
    let mut counters = Counters::default();
    let mut ready_input_fenced_clients = 0;
    for snapshot in snapshots {
        let snapshot = snapshot.lock().unwrap_or_else(|e| e.into_inner());
        ready_input_fenced_clients += u32::from(snapshot.fixture_input_fences_completed > 0);
        counters.merge(&snapshot);
    }
    let latency = counters.latency.json();
    let actions = if args.action_log && event == "final" {
        counters.actions.clone()
    } else {
        Vec::new()
    };
    let mut value = serde_json::to_value(&counters)?;
    let object = value
        .as_object_mut()
        .context("counter serialization is not an object")?;
    object.insert("event".into(), json!(event));
    object.insert(
        "success".into(),
        json!(error.is_none() && ready == args.clients),
    );
    object.insert("error".into(), json!(error));
    object.insert("clients".into(), json!(args.clients));
    object.insert("ready_clients".into(), json!(ready));
    object.insert(
        "walking_distance_scope".into(),
        json!("local submitted movement candidates, not server acknowledgements"),
    );
    object.insert("chunk_boundaries_scope".into(), json!("local candidate chunk changes; mechanics.players carries received cache-center evidence"));
    object.insert(
        "elapsed_seconds".into(),
        json!(started.elapsed().as_secs_f64()),
    );
    object.insert("action_ack_latency".into(), latency);
    object.insert("byte_metric".into(), json!("decoded_packet_body_bytes"));
    object.insert("byte_scope".into(), json!("play"));
    object.insert("view_distance".into(), json!(args.view_distance));
    object.insert("spacing".into(), json!(args.spacing));
    object.insert("group_size".into(), json!(args.group_size));
    object.insert(
        "protocol_version".into(),
        json!(mc_protocol::PROTOCOL_VERSION),
    );
    object.insert("release".into(), json!(mc_protocol::TARGET_RELEASE));
    object.insert("route_seed".into(), json!("bot_index * 0x9e3779b9"));
    object.insert("anchor_chunk_walk".into(), json!(args.anchor_chunk_walk));
    object.insert("street_routes".into(), json!(args.street_routes));
    object.insert("unassisted_login".into(), json!(args.unassisted_login));
    object.insert("name_prefix".into(), json!(args.name_prefix));
    object.insert(
        "success_scope".into(),
        json!("protocol/readiness transport, not catalog mechanic PASS"),
    );
    let ready_input_fence_required =
        args.open_loop && !args.unassisted_login && !args.mechanic_read_only;
    object.insert(
        "ready_input_fence_required".into(),
        json!(ready_input_fence_required),
    );
    object.insert(
        "ready_input_fenced_clients".into(),
        json!(ready_input_fenced_clients),
    );
    object.insert(
        "ready_input_fences_complete".into(),
        json!(ready_input_fence_required && ready_input_fenced_clients == args.clients),
    );
    if let Some(shared) = &args.mechanics {
        let epoch = *args.epoch.lock().unwrap_or_else(|e| e.into_inner());
        let ended = *args.ended.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = epoch.map(|epoch| ended.unwrap_or_else(|| epoch.instant.elapsed()));
        object.insert(
            "mechanics".into(),
            if matches!(event, "ready" | "schedule_complete" | "final" | "failure") {
                shared.lock().json(elapsed, args.mechanic_read_only)
            } else {
                shared.lock().summary(args.mechanic_read_only)
            },
        );
    }
    let chat_echo_clients = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .chat_echoes_received
                > 0
        })
        .count();
    object.insert("chat_echo_clients".into(), json!(chat_echo_clients));
    object.insert(
        "fixture_platform_verified".into(),
        json!(args.verify_platform && counters.fixture_platform_checks == u64::from(args.clients)),
    );
    if args.verify_platform {
        object.insert("fixture_platform_proof".into(), json!({
            "coordinates": [[0, 63, 0], [-32, 63, -32], [-32, 63, 32], [32, 63, -32], [32, 63, 32]],
            "block": "minecraft:grass_block", "feet_y": 64,
            "clearance": "two passable cells decoded from fresh chunk columns",
            "verified_clients": counters.fixture_platform_checks,
            "scope": "five sampled columns, not every platform cell",
        }));
    }
    if args.open_loop {
        let epoch = *args.epoch.lock().unwrap_or_else(|e| e.into_inner());
        let ended = *args.ended.lock().unwrap_or_else(|e| e.into_inner());
        let elapsed = epoch.map(|epoch| ended.unwrap_or_else(|| epoch.instant.elapsed()));
        let mut start = Duration::ZERO;
        let windows: Vec<_> = args.schedule.windows.iter().enumerate().map(|(index, window)| {
            let duration = Duration::from_millis(window.duration_ms);
            let local = elapsed.map(|elapsed| elapsed.saturating_sub(start).min(duration));
            let offered = |rate, clients| {
                if args.mechanic_read_only { 0 }
                else { local.map_or(0, |local| elapsed_slots(local, rate) * clients) }
            };
            let empty = WindowCounters::default();
            let totals = counters.windows.get(index).unwrap_or(&empty);
            let value = json!({
                "name": window.name, "duration_ms": window.duration_ms, "start_ms": start.as_millis(),
                "recovery": window.recovery,
                "movement": totals.movement.json(offered(window.movement_hz, u64::from(args.clients))),
                "edit": totals.edit.json(offered(window.edit_hz, u64::from(args.clients))),
                "command": totals.command.json(if args.mechanic_read_only { 0 } else { local.map_or(0, |local| window.command_slots(local)) }),
                "chat": totals.chat.json(offered(window.chat_hz, u64::from(args.clients))),
                "received": totals.received,
                "automatic_respawn": {"requests": totals.automatic_respawn_requests, "deferred": totals.automatic_respawn_deferred},
            });
            start += duration;
            value
        }).collect();
        let bots =
            if matches!(event, "final" | "failure") {
                snapshots.iter().enumerate().map(|(index, snapshot)| {
                let snapshot = snapshot.lock().unwrap_or_else(|e| e.into_inner());
                let mut received = WorkReceipts::default();
                for window in &snapshot.windows {
                    received.add_delta(&window.received, &WorkReceipts::default());
                }
                json!({"bot": index, "received": received, "chats_sent": snapshot.chats_sent,
                    "movements_sent": snapshot.movements_sent,
                    "walking_distance_blocks": snapshot.walking_distance_blocks,
                    "street_waypoints_reached": snapshot.street_waypoints_reached})
            }).collect::<Vec<_>>()
            } else {
                Vec::new()
            };
        object.insert("open_loop".into(), json!({
            "epoch_unix_ms": epoch.map(|epoch| epoch.unix_ms),
            "epoch_unix_ns": epoch.map(|epoch| epoch.unix_ns),
            "elapsed_ms": elapsed.map(|elapsed| elapsed.as_millis()),
            "schedule_complete": elapsed.is_some_and(|elapsed| elapsed >= args.schedule.duration()),
            "queue_capacity": 0, "pending_edit_capacity_per_bot": ACTION_CAP,
            "pending_chat_capacity_per_bot": ACTION_CAP, "poll_ms": 10,
            "slot_accounting": "wall elapsed, slots at n/rate for n>=1; missed slots are not replayed",
            "completed_scope": "edit: matching observed block update; chat: exact own server echo; movement/command: no completion oracle (zero)",
            "receipt_scope": "actual receive window; admission and sends are not mechanic completion",
            "windows": windows, "bots": bots,
        }));
    }
    if args.action_log && event == "final" {
        let observed: Vec<u64> = actions
            .iter()
            .filter_map(|record| record.observed_after_ms)
            .collect();
        object.insert(
            "action_log".into(),
            json!({
                "recorded": actions.len(),
                "cap_per_bot": ACTION_LOG_CAP,
                "event_cap_per_action": ACTION_EVENT_CAP,
                "observed_count": observed.len(),
                "observed_max_ms": observed.iter().max(),
                "unobserved": actions
                    .iter()
                    .filter(|record| record.observed_after_ms.is_none())
                    .collect::<Vec<_>>(),
            }),
        );
    }
    Ok(value)
}
fn emit(value: &Value) -> Result<()> {
    let mut stdout = io::stdout().lock();
    serde_json::to_writer(&mut stdout, value)?;
    writeln!(stdout)?;
    stdout.flush()?;
    Ok(())
}
fn save_report(args: &Args, value: &Value) -> Result<()> {
    let parent = args.report.parent().context("report path has no parent")?;
    std::fs::create_dir_all(parent)?;
    std::fs::write(&args.report, serde_json::to_vec_pretty(value)?)
        .with_context(|| format!("write report {}", args.report.display()))
}

/// A lost connection (socket error, EOF, or the server's own Disconnect),
/// as opposed to a decode/invariant failure inside the bot.
fn is_transport_drop(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<std::io::Error>().is_some() || {
            let text = cause.to_string();
            text == "peer closed" || text.starts_with("server disconnect:")
        }
    })
}

async fn wait_for_login_release(
    gate: &Barrier,
    release: &mut watch::Receiver<Option<u128>>,
) -> Result<u128> {
    gate.wait().await;
    loop {
        if let Some(unix_ns) = *release.borrow_and_update() {
            return Ok(unix_ns);
        }
        release
            .changed()
            .await
            .context("login release supervisor closed")?;
    }
}

fn control_reader() -> mpsc::Receiver<Result<RunControl>> {
    let (sender, receiver) = mpsc::channel(16);
    std::thread::spawn(move || {
        let stdin = io::stdin();
        let mut stdin = stdin.lock();
        let mut line = Vec::with_capacity(4097);
        loop {
            line.clear();
            let mut bounded = io::Read::take(io::Read::by_ref(&mut stdin), 4097);
            let result = io::BufRead::read_until(&mut bounded, b'\n', &mut line);
            let control = match result {
                Ok(0) => break,
                Ok(_) if line.len() > 4096 => {
                    Err(anyhow::anyhow!("stdin control exceeds 4096 bytes"))
                }
                Ok(_) => serde_json::from_slice(&line).context("invalid stdin run/stop control"),
                Err(error) => Err(error).context("read stdin control"),
            };
            let failed = control.is_err();
            if sender.blocking_send(control).is_err() || failed {
                break;
            }
        }
    });
    receiver
}

async fn run(mut args: Args) -> Result<()> {
    ensure!(
        args.report.is_absolute(),
        "--report must be an absolute path"
    );
    ensure!(
        !args.name_prefix.is_empty()
            && args.name_prefix.len() <= 11
            && args
                .name_prefix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
        "--name-prefix must be 1..=11 ASCII letters, digits or underscores"
    );
    if let Some(world) = &args.native_snapshot {
        let path = args
            .mechanic_spec
            .as_ref()
            .context("native snapshot requires mechanic spec")?;
        let spec: Value = serde_json::from_reader(std::fs::File::open(path)?)?;
        let mechanics = native_probe::native_control_snapshot(world, &spec)?;
        let report = json!({"event": "final", "success": true, "mechanics": mechanics});
        save_report(&args, &report)?;
        emit(&report)?;
        return Ok(());
    }
    if let Some(path) = &args.wire_trace_file {
        ensure!(path.is_absolute(), "--wire-trace-file must be absolute");
        ensure!(
            args.wire_trace_bots
                .iter()
                .all(|&index| index < args.clients),
            "--wire-trace-bots must name existing clients"
        );
        let sink = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .context("create exclusive diagnostic wire trace")?;
        args.wire_trace_sink = Some(Arc::new(Mutex::new(std::io::BufWriter::new(sink))));
    }
    if let Some(path) = &args.schedule_file {
        ensure!(path.is_absolute(), "--schedule-file must be absolute");
        let schedule: OpenLoopSchedule = serde_json::from_reader(std::fs::File::open(path)?)
            .context("load open-loop schedule")?;
        schedule.validate()?;
        args.schedule = Arc::new(schedule);
    }
    if let Some(path) = &args.mechanic_spec {
        ensure!(path.is_absolute(), "--mechanic-spec must be absolute");
        let mechanics =
            Mechanics::load(path, &args).context("load bounded mechanic specification")?;
        if mechanics.spec.snapshot_only {
            ensure!(
                !args.survival_on_run,
                "snapshot-only specification cannot transition to survival"
            );
            args.mechanic_read_only = true;
        }
        args.mechanics = Some(Arc::new(MechanicMutex::new(mechanics)));
    }
    if let Some(path) = &args.street_routes {
        ensure!(path.is_absolute(), "--street-routes must be absolute");
        args.street_paths = Arc::new(StreetRoutes::load(path, args.clients)?);
    }
    if let Some(path) = &args.anchors {
        let anchors: Vec<(i32, i32)> = serde_json::from_reader(std::fs::File::open(path)?)
            .context("load dry terrain anchors")?;
        ensure!(
            anchors.len() >= args.clients.div_ceil(args.group_size) as usize,
            "dry terrain fixture has fewer anchors than requested groups"
        );
        args.anchor_positions = Some(Arc::new(anchors));
    }
    if let Some(path) = &args.commands_file {
        ensure!(
            path.is_absolute(),
            "--commands-file must be an absolute path"
        );
        let commands: CommandSchedule = serde_json::from_reader(std::fs::File::open(path)?)
            .context("load fixture command schedule")?;
        ensure!(
            commands.recurring.iter().all(|entry| entry.every_ticks > 0),
            "recurring command interval must be positive"
        );
        ensure!(
            commands
                .setup
                .iter()
                .chain(commands.recurring.iter().flat_map(|entry| &entry.commands))
                .all(|command| !command.is_empty() && !command.contains(['\n', '\r'])),
            "fixture commands must be nonempty single lines"
        );
        args.commands = Arc::new(commands);
    }
    ensure!(
        !args.open_loop || args.commands.recurring.is_empty(),
        "open-loop recurring commands belong in the fixed window schedule; setup-only commands-file is pre-Run"
    );
    ensure!(
        !args.unassisted_login
            || (args.commands.setup.is_empty()
                && args.commands.recurring.is_empty()
                && args
                    .schedule
                    .windows
                    .iter()
                    .all(|window| window.command_hz == 0 && window.commands.is_empty())),
        "unassisted measured login forbids fixture commands"
    );
    let started = Instant::now();
    let snapshots: Vec<_> = (0..args.clients)
        .map(|_| Arc::new(Mutex::new(Counters::default())))
        .collect();
    let facts = match Facts::load() {
        Ok(facts) => Arc::new(facts),
        Err(error) => {
            let message = format!("load vanilla terrain registries: {error:#}");
            let report = aggregate(&snapshots, &args, 0, started, "failure", Some(&message))?;
            save_report(&args, &report)?;
            emit(&report)?;
            bail!("{message}");
        }
    };
    let admission = Arc::new(Semaphore::new(args.prepare_concurrency as usize));
    // All clients share one process: a single first-sighting set dedupes
    // cross-bot observations of the same arrow. Fresh per process start;
    // never cleared mid-run (respawn/chunk re-streams must not recount).
    let seen_projectiles = Arc::new(Mutex::new(HashSet::new()));
    let (stop_tx, stop_rx) = watch::channel(false);
    let (ready_tx, mut ready_rx) = mpsc::channel(args.clients as usize);
    let mut tasks = JoinSet::new();
    let login_gate = args
        .unassisted_login
        .then(|| Arc::new(Barrier::new(args.clients as usize + 1)));
    let login_release = args.unassisted_login.then(|| watch::channel(None::<u128>));
    for (index, snapshot) in snapshots.iter().enumerate() {
        let mut bot = Bot::new(
            index as u32,
            &args,
            Arc::clone(&facts),
            Arc::clone(snapshot),
            Arc::clone(&seen_projectiles),
        );
        let args = args.clone();
        let admission = Arc::clone(&admission);
        let ready_tx = ready_tx.clone();
        let mut stop = stop_rx.clone();
        let login_gate = login_gate.as_ref().map(Arc::clone);
        let mut login_release = login_release.as_ref().map(|(_, receiver)| receiver.clone());
        tasks.spawn(async move {
            let result: Result<()> = async {
                if let (Some(gate), Some(release)) = (login_gate, login_release.as_mut()) {
                    wait_for_login_release(&gate, release).await?;
                }
                let prepare_timeout = Duration::from_secs(args.prepare_seconds.max(1));
                let mut client = tokio::select! {
                    biased;
                    _ = stop.changed() => return Ok(()),
                    prepared = tokio::time::timeout(prepare_timeout, bot.prepare(&args, Arc::clone(&admission))) =>
                        prepared.with_context(|| format!("{}s target-window readiness deadline", args.prepare_seconds))??,
                };
                bot.publish();
                let ready_unix_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
                ready_tx.send(ready_unix_ns).await.context("readiness supervisor closed")?;
                loop {
                    let played = bot.play(&mut client, &mut stop, args.view_distance as i32).await;
                    let Err(error) = played else { return Ok(()) };
                    // Only a lost connection is retried; decode/invariant
                    // failures stay fatal so a protocol regression is visible.
                    if !args.reconnect || *stop.borrow() || !is_transport_drop(&error) {
                        return Err(error);
                    }
                    bot.metrics.disconnects += 1;
                    if let Some(index) = bot.current_window() {
                        bot.metrics.windows[index].received.disconnects += 1;
                    }
                    bot.publish();
                    emit(&json!({"event": "bot_reconnect", "bot": bot.index, "error": format!("{error:#}")}))?;
                    // Chunks seen belong to the dropped connection.
                    bot.seen.clear();
                    client = tokio::select! {
                        biased;
                        _ = stop.changed() => return Ok(()),
                        prepared = tokio::time::timeout(prepare_timeout, bot.prepare(&args, Arc::clone(&admission))) =>
                            prepared.with_context(|| format!("{}s reconnect readiness deadline", args.prepare_seconds))??,
                    };
                }
            }.await;
            if result.is_err() {
                bot.metrics.disconnects += 1;
                if let Some(index) = bot.current_window() {
                    bot.metrics.windows[index].received.disconnects += 1;
                }
            }
            bot.publish();
            result.with_context(|| format!("bot {}", bot.index))
        });
    }
    if let (Some(gate), Some((release, _))) = (&login_gate, &login_release) {
        // Every task must be parked before the single producer admits login.
        // No connect/handshake or per-bot readiness response can pace admission.
        gate.wait().await;
        let unix_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        release.send_replace(Some(unix_ns));
        emit(&json!({"event": "login_release", "unix_ns": unix_ns, "clients": args.clients}))?;
    }
    drop(ready_tx);
    let mut ready = 0u32;
    let mut last_ready_unix_ns = 0u128;
    let mut readiness_channel_open = true;
    let mut samples = tokio::time::interval(Duration::from_secs(1));
    samples.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    let signal = tokio::signal::ctrl_c();
    tokio::pin!(signal);
    let mut failure = None;
    let mut controls = if args.open_loop {
        Some(control_reader())
    } else {
        None
    };
    let mut schedule_announced = false;
    loop {
        tokio::select! {
            biased;
            result = &mut signal => {
                if let Err(error) = result { failure = Some(format!("SIGINT handler: {error}")); }
                else if ready != args.clients { failure = Some(format!("interrupted before readiness: {ready}/{}", args.clients)); }
                break;
            }
            control = async { controls.as_mut().expect("enabled controls").recv().await }, if controls.is_some() => {
                match control {
                    Some(Ok(RunControl::Run {})) => {
                        if ready != args.clients {
                            failure = Some(format!("run before readiness: {ready}/{}", args.clients));
                            break;
                        }
                        if args.epoch.lock().unwrap_or_else(|e| e.into_inner()).is_some() {
                            failure = Some("run epoch already established".into());
                            break;
                        }
                        let mut baseline = args.mechanics.as_ref().map(|shared| shared.lock());
                        let instant = if let Some(state) = baseline.as_deref_mut() {
                            state.begin_measurement()?
                        } else {
                            Instant::now()
                        };
                        let unix_ns = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
                        let epoch = RunEpoch {
                            instant,
                            unix_ms: (unix_ns / 1_000_000) as u64,
                            unix_ns,
                        };
                        *args.epoch.lock().unwrap_or_else(|e| e.into_inner()) = Some(epoch);
                        drop(baseline);
                        emit(&aggregate(&snapshots, &args, ready, started, "running", None)?)?;
                    }
                    Some(Ok(RunControl::Stop {})) => {
                        if ready != args.clients { failure = Some("stopped before readiness".into()); }
                        break;
                    }
                    Some(Err(error)) => { failure = Some(format!("{error:#}")); break; }
                    None => controls = None,
                }
            }
            result = tasks.join_next() => {
                match result {
                    Some(Ok(Err(error))) => failure = Some(format!("{error:#}")),
                    Some(Err(error)) => failure = Some(format!("bot task: {error}")),
                    _ => failure = Some("bot task stopped before SIGINT".into()),
                }
                break;
            }
            notification = ready_rx.recv(), if readiness_channel_open => {
                if let Some(ready_unix_ns) = notification {
                    ready += 1;
                    last_ready_unix_ns = last_ready_unix_ns.max(ready_unix_ns);
                    if ready == args.clients {
                        let mut event = aggregate(&snapshots, &args, ready, started, "ready", None)?;
                        event["ready_unix_ns"] = json!(last_ready_unix_ns);
                        emit(&event)?;
                    }
                } else { readiness_channel_open = false; }
            }
            _ = samples.tick() => {
                let complete = args.epoch.lock().unwrap_or_else(|e| e.into_inner())
                    .is_some_and(|epoch| epoch.instant.elapsed() >= args.schedule.duration());
                let event = if args.open_loop && complete && !schedule_announced {
                    schedule_announced = true;
                    if let Some(shared) = &args.mechanics {
                        shared.lock().measurement_closed = true;
                    }
                    "schedule_complete"
                } else { "sample" };
                emit(&aggregate(&snapshots, &args, ready, started, event, None)?)?;
            },
        }
    }
    if let Some(epoch) = *args.epoch.lock().unwrap_or_else(|e| e.into_inner()) {
        *args.ended.lock().unwrap_or_else(|e| e.into_inner()) = Some(epoch.instant.elapsed());
        if let Some(shared) = &args.mechanics {
            shared.lock().measurement_closed = true;
        }
    }
    stop_tx.send_replace(true);
    // Cancellation covers handshake/readiness and Play reads. Bound shutdown in
    // case a congested socket is stuck writing a packet inside an event handler.
    let drain = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(result) = tasks.join_next().await {
            match result {
                Ok(Err(error)) if failure.is_none() => failure = Some(format!("{error:#}")),
                Err(error) if failure.is_none() => failure = Some(format!("bot task: {error}")),
                _ => {}
            }
        }
    })
    .await;
    if drain.is_err() {
        tasks.abort_all();
        if failure.is_none() {
            failure = Some("bot shutdown exceeded 5s".into());
        }
        while tasks.join_next().await.is_some() {}
    }
    if let Some(sink) = &args.wire_trace_sink {
        sink.lock()
            .unwrap_or_else(|e| e.into_inner())
            .flush()
            .context("flush diagnostic wire trace")?;
    }
    let event = if failure.is_some() {
        "failure"
    } else {
        "final"
    };
    let report = aggregate(&snapshots, &args, ready, started, event, failure.as_deref())?;
    save_report(&args, &report)?;
    emit(&report)?;
    if let Some(message) = failure {
        bail!("{message}");
    }
    Ok(())
}

#[tokio::main(flavor = "multi_thread", worker_threads = 4)]
async fn main() -> Result<()> {
    run(Args::parse()).await
}

#[cfg(test)]
#[path = "player_scale_bots_tests.rs"]
mod tests;
