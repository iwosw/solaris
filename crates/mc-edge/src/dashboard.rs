//! Read-only operator dashboard for the v2 edge server.
//!
//! A tiny tokio TCP HTTP/1.1 endpoint with a request-head cap,
//! one-request-per-connection, `GET /stats`, `GET /profile`, and streaming
//! `GET /load-metrics` routes (404 elsewhere). Payloads stay edge-local.
//!
//! The existing `/stats` rolling 1200 samples remain independent of the
//! preallocated 16384-sample `/load-metrics` ring. Stream `?after_tick=N`
//! with the last consumed tick to retain arbitrarily long wall windows.
//! `&wait=true&min_samples=20` waits on pushed sample notifications, batching
//! at least 20 newer samples (range 1..=1200; default 1). A single 10-second
//! deadline returns currently available samples with `wait_timed_out=true`;
//! nonwaiting requests and satisfied waits carry `false`. Overrun returns
//! immediately even below the requested sample threshold. `overrun`
//! reports overwritten requested samples; noncontiguous tick IDs or increases
//! in `source_skipped_ticks_total` also invalidate affected windows.
//!
//! Raw samples retain the core's original UNIX-nanosecond wall start (before
//! input drains), not edge receipt time or a reconstructed 50 ms cadence.
//! Use `wall_us` for full active-owner tick percentiles; `total_us` retains
//! the narrower `run_tick` + journal cost. Both durations are monotonic;
//! wall timestamps can step with the system clock. Queue gauges are request-
//! time, non-atomic observations between their capture/completion timestamps,
//! not historical tick samples or socket/kernel buffered-byte measurements.

use std::collections::VecDeque;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::Notify;

use mc_core::boundary::TickTimes;

use crate::server::ConnectionRegistry;

/// Bounded storage, about 13.6 minutes at 20 TPS; stream longer windows.
const LOAD_SAMPLE_CAPACITY: usize = 16384;
/// A long poll waits on pushes, with this deadline as its only timer.
const LOAD_WAIT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a dashboard peer may take to deliver a complete request head.
const READ_TIMEOUT: Duration = Duration::from_secs(5);
/// Request-head read cap: larger heads are rejected with 431.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// World identity facts served under `world`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardWorld {
    /// Server display name from `[server] name`.
    pub name: String,
    /// Message of the day from `[server] motd`.
    pub motd: String,
    /// World seed.
    pub seed: i64,
    /// Chunk view radius.
    pub view_distance: u8,
    /// Chunk simulation radius.
    pub simulation_distance: u8,
    /// Maximum concurrent Play sessions.
    pub max_players: u32,
}

/// Connected-player roster served under `players`.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardPlayers {
    /// Current Play session count.
    pub count: usize,
    /// Configured cap.
    pub max: u32,
    /// Online player names, sorted.
    pub names: Vec<String>,
}

/// Counters committed together after one pump publish.
#[derive(Default)]
pub(crate) struct PumpTick {
    pub tick: u64,
    pub core_us: u64,
    pub publish_us: u64,
    pub bytes_written: u64,
    pub reliable_drops: u64,
    pub chunks: DashboardChunks,
    pub entities: DashboardEntities,
    pub players: Option<DashboardPlayers>,
}

/// Percentiles over the last 1200 ticks, in microseconds.
#[derive(Debug, Clone, Copy, Serialize, Default)]
pub struct TickLatency {
    /// Tick that produced the newest sample in the window.
    pub source_tick: u64,
    /// p50, µs.
    pub p50_us: u64,
    /// p95, µs.
    pub p95_us: u64,
    /// p99, µs.
    pub p99_us: u64,
    /// Maximum in the window, µs.
    pub max_us: u64,
    /// Samples in the window.
    pub samples: usize,
}

/// Chunk pipeline counters.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardChunks {
    /// Chunks resident in the newest core snapshot.
    pub loaded_total: usize,
}

/// Entity population totals.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardEntities {
    /// Simulated entities, players included.
    pub total: usize,
}

/// Network throughput and reliability counters.
#[derive(Debug, Clone, Serialize, Default)]
pub struct DashboardNetwork {
    /// Total bytes written to Play connections.
    pub bytes_written: u64,
    /// Reliable-queue overflows that disconnected a client.
    pub reliable_drops: u64,
}

/// Actual production component execution counters, not submissions or bot writes.
#[derive(Debug, Clone, Serialize)]
pub struct PluginStats {
    pub runtime_ready: bool,
    pub loaded_packages: usize,
    pub wasm_invocations: u64,
    pub host_operations: u64,
    pub command_results: u64,
}

/// The complete `/stats` payload.
#[derive(Debug, Clone, Serialize)]
pub struct StatsSnapshot {
    /// Crate version string.
    pub version: String,
    /// Seconds since the edge process started.
    pub uptime_secs: u64,
    /// Latest tick number observed by the stats rollup.
    pub simulation_tick: u64,
    /// World identity facts.
    pub world: DashboardWorld,
    /// Connected players.
    pub players: DashboardPlayers,
    /// Core ticks per second over the sample window, from published tick
    /// numbers, so ticks the pump skipped while lagging still count.
    pub tps: f64,
    /// Server-side tick cost (core `run_tick` + journal), from `TickOutput.times`.
    pub tick: TickLatency,
    /// Edge pump fanout time per tick (plan + route to every viewer).
    pub publish: TickLatency,
    /// Core ticks the pump skipped (`broadcast` lag) since start; nonzero
    /// means fanout, not the core, is behind.
    pub pump_skipped_ticks: u64,
    /// Chunk pipeline counters.
    pub chunks: DashboardChunks,
    /// Entity population totals.
    pub entities: DashboardEntities,
    /// Network counters.
    pub network: DashboardNetwork,
    /// Absent when no production component deployment is configured.
    pub plugin: Option<PluginStats>,
    /// Autoscale is not implemented on the edge; always `false`.
    pub autoscale: bool,
    /// Operator warnings (empty unless the backend reports trouble).
    pub warnings: Vec<String>,
}

/// Original per-tick phase costs, without reconstructing missing samples.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LoadPhaseTimes {
    pub input_us: u64,
    pub chunks_us: u64,
    pub entity_us: u64,
    pub merge_us: u64,
    pub block_us: u64,
    pub commit_us: u64,
    pub snapshot_us: u64,
    pub views_us: u64,
    pub publish_us: u64,
    pub plugins_us: u64,
    pub coordination_us: u64,
    pub journal_build_us: u64,
    pub journal_queue_us: u64,
    pub generated_us: u64,
    pub owner_pre_us: u64,
    pub owner_post_us: u64,
}

/// A completed active core iteration, distinct from edge fanout.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct LoadTickSample {
    pub tick: u64,
    pub wall_start_unix_ns: u64,
    pub wall_us: u64,
    pub total_us: u64,
    pub phase_us: LoadPhaseTimes,
}

/// Real send-channel gauges. `capacity()`, unlike a receiver length, includes
/// outstanding reservations: queued and reserved slots cannot be separated.
#[derive(Debug, Default, Serialize)]
pub struct LoadQueueGauge {
    pub capacity_slots: usize,
    pub queued_or_reserved_slots: usize,
    pub max_viewer_queued_or_reserved_slots: usize,
    pub queued_slots: Option<usize>,
    pub reserved_slots: Option<usize>,
    pub queued_bytes: Option<u64>,
    pub reserved_bytes: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct LoadQueueCapabilities {
    pub queued_or_reserved_slots: bool,
    pub queued_slots: bool,
    pub reserved_slots: bool,
    pub queued_bytes: bool,
    pub reserved_bytes: bool,
}

/// Aggregation over currently registered Play connections. Channel writers
/// may drain between observations; this is not a globally atomic snapshot.
#[derive(Debug, Serialize)]
pub struct LoadQueues {
    pub captured_unix_ns: u64,
    pub completed_unix_ns: u64,
    pub viewers: usize,
    pub capabilities: LoadQueueCapabilities,
    pub reliable: LoadQueueGauge,
    pub world: LoadQueueGauge,
}

/// Versioned streaming telemetry; a null queue object means unavailable.
#[derive(Debug, Serialize)]
pub struct LoadMetricsSnapshot {
    pub schema_version: u32,
    pub captured_unix_ns: u64,
    pub instance_started_unix_ns: u64,
    pub capacity: usize,
    pub after_tick: Option<u64>,
    pub oldest_tick: Option<u64>,
    pub latest_tick: Option<u64>,
    pub overrun: bool,
    /// True only when a requested push-driven wait reached its deadline.
    pub wait_timed_out: bool,
    pub overwritten_samples_total: u64,
    pub source_skipped_ticks_total: u64,
    pub samples: Vec<LoadTickSample>,
    pub queues: Option<LoadQueues>,
}

struct LoadMetricsInner {
    samples: VecDeque<LoadTickSample>,
    last_evicted_tick: Option<u64>,
    overwritten_samples_total: u64,
    source_skipped_ticks_total: u64,
}

/// Mutable dashboard state. Metrics have an independent lock and never
/// serialize on the core thread. All producer storage is allocated at setup.
pub struct DashboardState {
    inner: parking_lot::Mutex<StateInner>,
    started: Instant,
    instance_started_unix_ns: u64,
    load_metrics: Mutex<LoadMetricsInner>,
    load_changed: Notify,
    queue_source: OnceLock<Arc<ConnectionRegistry>>,
    plugin_source: OnceLock<Arc<mc_core::Core>>,
}

#[derive(Debug)]
struct StateInner {
    world: DashboardWorld,
    players: DashboardPlayers,
    simulation_tick: u64,
    /// Pump arrivals `(tick, at)` for TPS; ring buffer capped at 1200.
    arrivals: VecDeque<(u64, Instant)>,
    /// Core tick cost per tick, µs; ring buffer capped at 1200.
    tick_us: VecDeque<u64>,
    /// Pump publish time per tick, µs; ring buffer capped at 1200.
    publish_us: VecDeque<u64>,
    pump_skipped_ticks: u64,
    chunks: DashboardChunks,
    entities: DashboardEntities,
    network: DashboardNetwork,
    warnings: Vec<String>,
}

impl DashboardState {
    /// Create state with fixed world facts; counters start at zero.
    pub fn new(world: DashboardWorld) -> Self {
        Self {
            inner: parking_lot::Mutex::new(StateInner {
                world,
                players: DashboardPlayers::default(),
                simulation_tick: 0,
                arrivals: VecDeque::new(),
                tick_us: VecDeque::new(),
                publish_us: VecDeque::new(),
                pump_skipped_ticks: 0,
                chunks: DashboardChunks::default(),
                entities: DashboardEntities::default(),
                network: DashboardNetwork::default(),
                warnings: Vec::new(),
            }),
            started: Instant::now(),
            instance_started_unix_ns: unix_ns(),
            load_metrics: Mutex::new(LoadMetricsInner {
                samples: VecDeque::with_capacity(LOAD_SAMPLE_CAPACITY),
                last_evicted_tick: None,
                overwritten_samples_total: 0,
                source_skipped_ticks_total: 0,
            }),
            load_changed: Notify::new(),
            queue_source: OnceLock::new(),
            plugin_source: OnceLock::new(),
        }
    }

    /// Attach the existing send-channel registry once; gauges are read only
    /// when an operator requests them, never aggregated on the tick path.
    pub fn set_queue_source(&self, connections: Arc<ConnectionRegistry>) {
        let _ = self.queue_source.set(connections);
    }

    /// Read real component counters on demand, without a tick-path producer.
    pub fn set_plugin_source(&self, core: Arc<mc_core::Core>) {
        let _ = self.plugin_source.set(core);
    }

    fn plugin_snapshot(&self) -> Option<PluginStats> {
        self.plugin_source.get().map(|core| PluginStats {
            runtime_ready: true,
            loaded_packages: core.plugin_packages().len(),
            wasm_invocations: mc_plugin_host::wasm_invocations(),
            host_operations: core.plugin_host_operations(),
            command_results: core.plugin_command_results(),
        })
    }

    /// Record a timing-lane sample without allocation. Its independent
    /// subscriber does not wait for edge fanout and preserves source gaps.
    pub fn record_load_tick(&self, tick: u64, times: TickTimes) {
        let Ok(mut inner) = self.load_metrics.lock() else {
            return;
        };
        if inner.samples.len() == LOAD_SAMPLE_CAPACITY {
            inner.last_evicted_tick = inner.samples.pop_front().map(|sample| sample.tick);
            inner.overwritten_samples_total = inner.overwritten_samples_total.saturating_add(1);
        }
        inner.samples.push_back(LoadTickSample {
            tick,
            wall_start_unix_ns: times.wall_start_unix_ns,
            wall_us: times.wall_us,
            total_us: times.total_us,
            phase_us: LoadPhaseTimes {
                input_us: times.input_us,
                chunks_us: times.chunks_us,
                entity_us: times.entity_us,
                merge_us: times.merge_us,
                block_us: times.block_us,
                commit_us: times.commit_us,
                snapshot_us: times.snapshot_us,
                views_us: times.views_us,
                publish_us: times.publish_us,
                plugins_us: times.plugins_us,
                coordination_us: times.coordination_us,
                journal_build_us: times.journal_build_us,
                journal_queue_us: times.journal_queue_us,
                generated_us: times.generated_us,
                owner_pre_us: times.owner_pre_us,
                owner_post_us: times.owner_post_us,
            },
        });
        drop(inner);
        self.load_changed.notify_waiters();
    }

    /// Count timing broadcast loss independently of world pump lag.
    pub fn record_load_skipped(&self, skipped: u64) {
        if let Ok(mut inner) = self.load_metrics.lock() {
            inner.source_skipped_ticks_total =
                inner.source_skipped_ticks_total.saturating_add(skipped);
        }
        self.load_changed.notify_waiters();
    }

    /// Copy only requested raw samples while locked, then read live queues.
    /// A poisoned metrics lock is unavailable, never a fabricated empty ring.
    pub fn load_snapshot(&self, after_tick: Option<u64>) -> Option<LoadMetricsSnapshot> {
        let captured_unix_ns = unix_ns();
        let inner = self.load_metrics.lock().ok()?;
        let snapshot = LoadMetricsSnapshot {
            schema_version: 1,
            captured_unix_ns,
            instance_started_unix_ns: self.instance_started_unix_ns,
            capacity: LOAD_SAMPLE_CAPACITY,
            after_tick,
            oldest_tick: inner.samples.front().map(|sample| sample.tick),
            latest_tick: inner.samples.back().map(|sample| sample.tick),
            overrun: after_tick.is_some_and(|after| {
                inner
                    .last_evicted_tick
                    .is_some_and(|evicted| after < evicted)
            }),
            wait_timed_out: false,
            overwritten_samples_total: inner.overwritten_samples_total,
            source_skipped_ticks_total: inner.source_skipped_ticks_total,
            samples: inner
                .samples
                .iter()
                .filter(|sample| after_tick.is_none_or(|after| sample.tick > after))
                .copied()
                .collect(),
            queues: None,
        };
        drop(inner);
        Some(LoadMetricsSnapshot {
            queues: self
                .queue_source
                .get()
                .and_then(|source| source.load_queues()),
            ..snapshot
        })
    }

    /// Register before checking the ring to avoid lost wakeups. Neither this
    /// wait nor the producer allocates per tick; only readers create futures.
    async fn wait_for_load_samples(
        &self,
        after_tick: Option<u64>,
        min_samples: usize,
    ) -> Option<()> {
        let notified = self.load_changed.notified();
        tokio::pin!(notified);
        loop {
            notified.as_mut().enable();
            {
                let inner = self.load_metrics.lock().ok()?;
                let overrun = after_tick.is_some_and(|after| {
                    inner
                        .last_evicted_tick
                        .is_some_and(|evicted| after < evicted)
                });
                let available = inner
                    .samples
                    .iter()
                    .rev()
                    .take_while(|sample| after_tick.is_none_or(|after| sample.tick > after))
                    .take(min_samples)
                    .count();
                if overrun || available >= min_samples {
                    return Some(());
                }
            }
            notified.as_mut().await;
            notified.set(self.load_changed.notified());
        }
    }

    /// Commit one published tick and its counters in one critical section.
    pub(crate) fn record_pump_tick(&self, sample: PumpTick) {
        let now = Instant::now();
        let mut inner = self.inner.lock();
        inner.simulation_tick = inner.simulation_tick.max(sample.tick);
        if inner.arrivals.len() >= 1200 {
            inner.arrivals.pop_front();
        }
        inner.arrivals.push_back((sample.tick, now));
        push_window(&mut inner.tick_us, sample.core_us);
        push_window(&mut inner.publish_us, sample.publish_us);
        inner.network.bytes_written = sample.bytes_written;
        inner.network.reliable_drops = sample.reliable_drops;
        inner.chunks = sample.chunks;
        inner.entities = sample.entities;
        if let Some(players) = sample.players {
            inner.players = players;
        }
    }

    /// Count core ticks the pump skipped after a `broadcast` lag.
    pub fn record_pump_skipped(&self, ticks: u64) {
        let mut inner = self.inner.lock();
        inner.pump_skipped_ticks = inner.pump_skipped_ticks.saturating_add(ticks);
    }

    /// Replace the operator warning list.
    pub fn set_warnings(&self, warnings: Vec<String>) {
        let mut inner = self.inner.lock();
        inner.warnings = warnings;
    }

    /// Render the current `/stats` snapshot.
    pub fn snapshot(&self) -> StatsSnapshot {
        let uptime_secs = self.started.elapsed().as_secs();
        let inner = self.inner.lock();
        let tick = summarize(&inner.tick_us, inner.simulation_tick);
        let publish = summarize(&inner.publish_us, inner.simulation_tick);
        let tps = tps_of(&inner.arrivals);
        StatsSnapshot {
            version: env!("CARGO_PKG_VERSION").to_owned(),
            uptime_secs,
            simulation_tick: inner.simulation_tick,
            world: inner.world.clone(),
            players: inner.players.clone(),
            tps,
            pump_skipped_ticks: inner.pump_skipped_ticks,
            tick,
            publish,
            chunks: inner.chunks.clone(),
            entities: inner.entities.clone(),
            network: inner.network.clone(),
            plugin: self.plugin_snapshot(),
            autoscale: false,
            warnings: inner.warnings.clone(),
        }
    }
}

/// UNIX wall timestamp, with zero explicitly representing an unavailable clock.
pub(crate) fn unix_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos().min(u128::from(u64::MAX)) as u64)
        .unwrap_or(0)
}

fn push_window(window: &mut VecDeque<u64>, value: u64) {
    if window.len() >= 1200 {
        window.pop_front();
    }
    window.push_back(value);
}

fn sorted_us(intervals: &VecDeque<u64>) -> Vec<u64> {
    let mut v: Vec<u64> = intervals.iter().copied().collect();
    v.sort_unstable();
    v
}

fn percentile(sorted: &[u64], q: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (q * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summarize(intervals: &VecDeque<u64>, source_tick: u64) -> TickLatency {
    let sorted = sorted_us(intervals);
    TickLatency {
        source_tick,
        p50_us: percentile(&sorted, 0.50),
        p95_us: percentile(&sorted, 0.95),
        p99_us: percentile(&sorted, 0.99),
        max_us: sorted.last().copied().unwrap_or(0),
        samples: sorted.len(),
    }
}

/// Core TPS over the window: tick-number delta / elapsed time, clamped to
/// [0, 20]. Fewer than two arrivals report 0 so a fresh boot never claims
/// full rate.
fn tps_of(arrivals: &VecDeque<(u64, Instant)>) -> f64 {
    let (Some(&(first_tick, first_at)), Some(&(last_tick, last_at))) =
        (arrivals.front(), arrivals.back())
    else {
        return 0.0;
    };
    let secs = last_at.saturating_duration_since(first_at).as_secs_f64();
    if secs <= 0.0 {
        return 0.0;
    }
    (last_tick.saturating_sub(first_tick) as f64 / secs).clamp(0.0, 20.0)
}

/// Serve the dashboard on `addr:port` until the process exits. Binding
/// refuses a non-loopback address unless `allow_remote` is set; bind
/// failure is logged and the task returns.
pub async fn serve(addr: String, port: u16, state: Arc<DashboardState>) {
    serve_inner(addr, port, state, false).await;
}

/// Test/embedding entry: like [`serve`] but returns whether the listener
/// bound, so tests can assert the `allow_remote` gate.
pub async fn serve_bound(addr: String, port: u16, state: Arc<DashboardState>) -> bool {
    serve_inner(addr, port, state, true).await
}

async fn serve_inner(addr: String, port: u16, state: Arc<DashboardState>, report: bool) -> bool {
    let target = format!("{addr}:{port}");
    let Ok(sock) = target.parse::<SocketAddr>() else {
        tracing::warn!(target = %target, "dashboard: invalid bind address");
        return false;
    };
    if !sock.ip().is_loopback() {
        tracing::warn!(target = %target, "dashboard: non-loopback bind refused without allow_remote");
        return false;
    }
    let Ok(listener) = TcpListener::bind(sock).await else {
        tracing::warn!(target = %target, "dashboard: bind failed");
        return false;
    };
    tracing::info!(target = %target, "dashboard bound and listening");
    if report {
        tokio::spawn(run(listener, state));
        return true;
    }
    run(listener, state).await;
    true
}

async fn run(listener: TcpListener, state: Arc<DashboardState>) {
    loop {
        let Ok((mut stream, _)) = listener.accept().await else {
            continue;
        };
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            serve_connection(&mut stream, &state).await;
        });
    }
}

/// Serve exactly one request on one connection, then close it.
async fn serve_connection<S>(stream: &mut S, state: &Arc<DashboardState>)
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let Ok(head) = read_head(stream).await else {
        return;
    };
    let Some(request) = parse_head(&head) else {
        let body = b"bad request";
        let _ = write_response(stream, "400 Bad Request", "text/plain", body).await;
        return;
    };
    if request.method != "GET" {
        let body = b"method not allowed";
        let _ = write_response_allow(stream, "405 Method Not Allowed", body).await;
        return;
    }
    match request.path.as_str() {
        "/stats" => {
            let snapshot = state.snapshot();
            let Ok(body) = serde_json::to_vec(&snapshot) else {
                let _ = write_response(
                    stream,
                    "500 Internal Server Error",
                    "text/plain",
                    b"encode error",
                )
                .await;
                return;
            };
            let _ = write_response(stream, "200 OK", "application/json", &body).await;
        }
        "/load-metrics" => {
            let Ok(query) = parse_load_query(request.query.as_deref()) else {
                let _ = write_response(
                    stream,
                    "400 Bad Request",
                    "text/plain",
                    b"invalid metrics query",
                )
                .await;
                return;
            };
            let wait_timed_out = if query.wait {
                tokio::time::timeout(
                    LOAD_WAIT_TIMEOUT,
                    state.wait_for_load_samples(query.after_tick, query.min_samples),
                )
                .await
                .is_err()
            } else {
                false
            };
            let Some(mut snapshot) = state.load_snapshot(query.after_tick) else {
                let _ = write_response(
                    stream,
                    "503 Service Unavailable",
                    "text/plain",
                    b"metrics unavailable",
                )
                .await;
                return;
            };
            snapshot.wait_timed_out = wait_timed_out;
            let Ok(body) = serde_json::to_vec(&snapshot) else {
                let _ = write_response(
                    stream,
                    "500 Internal Server Error",
                    "text/plain",
                    b"encode error",
                )
                .await;
                return;
            };
            let _ = write_response(stream, "200 OK", "application/json", &body).await;
        }
        "/profile" => {
            let snapshot = state.snapshot();
            let now_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let mut value = serde_json::to_value(&snapshot).unwrap_or_default();
            if let Some(obj) = value.as_object_mut() {
                obj.insert("profile_schema".to_owned(), serde_json::json!(1));
                obj.insert("captured_unix_ms".to_owned(), serde_json::json!(now_ms));
            }
            let body = serde_json::to_vec(&value).unwrap_or_default();
            let _ = write_response(stream, "200 OK", "application/json", &body).await;
        }
        _ => {
            let body = b"not found";
            let _ = write_response(stream, "404 Not Found", "text/plain", body).await;
        }
    }
}

struct Request {
    method: String,
    path: String,
    query: Option<String>,
}

/// Parse a request head. Headers are ignored; the query is preserved for
/// `/load-metrics` while other routes continue to ignore it.
fn parse_head(head: &str) -> Option<Request> {
    let line = head.lines().next()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_owned();
    let target = parts.next()?;
    let (path, query) = target
        .split_once('?')
        .map_or((target, None), |(path, query)| {
            (path, Some(query.to_owned()))
        });
    Some(Request {
        method,
        path: path.to_owned(),
        query,
    })
}

#[derive(Debug, PartialEq, Eq)]
struct LoadMetricsQuery {
    after_tick: Option<u64>,
    wait: bool,
    min_samples: usize,
}

fn parse_load_query(query: Option<&str>) -> Result<LoadMetricsQuery, ()> {
    let mut after_tick = None;
    let mut wait = None;
    let mut min_samples = None;
    for parameter in query.into_iter().flat_map(|query| query.split('&')) {
        if parameter.is_empty() {
            continue;
        }
        let (key, value) = parameter.split_once('=').ok_or(())?;
        match key {
            "wait" if wait.is_none() => {
                wait = Some(match value {
                    "true" => true,
                    "false" => false,
                    _ => return Err(()),
                });
            }
            "after_tick" if after_tick.is_none() => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(());
                }
                after_tick = Some(value.parse().map_err(|_| ())?);
            }
            "min_samples" if min_samples.is_none() => {
                if value.is_empty() || !value.bytes().all(|byte| byte.is_ascii_digit()) {
                    return Err(());
                }
                let samples = value.parse::<usize>().map_err(|_| ())?;
                if !(1..=1200).contains(&samples) {
                    return Err(());
                }
                min_samples = Some(samples);
            }
            _ => return Err(()),
        }
    }
    Ok(LoadMetricsQuery {
        after_tick,
        wait: wait.unwrap_or(false),
        min_samples: min_samples.unwrap_or(1),
    })
}

/// Read bytes until the CRLFCRLF head terminator, bounded by
/// [`MAX_HEAD_BYTES`]. Peer EOF or an oversized head is an error.
async fn read_head<S>(stream: &mut S) -> Result<String, ()>
where
    S: AsyncRead + Unpin,
{
    let mut buf = Vec::with_capacity(512);
    let mut chunk = [0_u8; 512];
    let deadline = tokio::time::sleep(READ_TIMEOUT);
    tokio::pin!(deadline);
    loop {
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            break;
        }
        if buf.len() > MAX_HEAD_BYTES {
            return Err(());
        }
        tokio::select! {
            biased;
            _ = &mut deadline => return Err(()),
            read = stream.read(&mut chunk) => {
                let n = read.map_err(|_| ())?;
                if n == 0 {
                    return Err(());
                }
                buf.extend_from_slice(&chunk[..n]);
            }
        }
    }
    String::from_utf8(buf).map_err(|_| ())
}

async fn write_response<S>(
    stream: &mut S,
    status: &str,
    content_type: &str,
    body: &[u8],
) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let head = format!(
        "HTTP/1.1 {status}\r\ncontent-type: {content_type}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

async fn write_response_allow<S>(stream: &mut S, status: &str, body: &[u8]) -> std::io::Result<()>
where
    S: AsyncWrite + Unpin,
{
    let head = format!(
        "HTTP/1.1 {status}\r\nallow: GET\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(head.as_bytes()).await?;
    stream.write_all(body).await?;
    stream.flush().await
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::duplex;

    fn test_state() -> Arc<DashboardState> {
        Arc::new(DashboardState::new(DashboardWorld {
            name: "t".to_owned(),
            motd: "m".to_owned(),
            seed: 81,
            view_distance: 4,
            simulation_distance: 3,
            max_players: 20,
        }))
    }

    #[test]
    fn load_samples_preserve_source_times_and_gaps_without_changing_stats() {
        let state = test_state();
        let times = TickTimes {
            wall_start_unix_ns: 1_800_000_000_123_456_789,
            wall_us: 99,
            total_us: 45,
            entity_us: 11,
            merge_us: 12,
            block_us: 13,
            snapshot_us: 14,
            views_us: 15,
            input_us: 16,
            chunks_us: 17,
            commit_us: 18,
            publish_us: 19,
            plugins_us: 20,
            coordination_us: 21,
            journal_build_us: 22,
            journal_queue_us: 23,
            generated_us: 24,
            owner_pre_us: 25,
            owner_post_us: 26,
        };
        state.record_load_tick(41, times);
        state.record_load_skipped(2);
        state.record_load_tick(
            44,
            TickTimes {
                wall_start_unix_ns: times.wall_start_unix_ns + 987_654_321,
                ..times
            },
        );
        let snapshot = state.load_snapshot(Some(40)).unwrap();
        assert_eq!(snapshot.oldest_tick, Some(41));
        assert_eq!(snapshot.latest_tick, Some(44));
        assert_eq!(snapshot.source_skipped_ticks_total, 2);
        assert!(!snapshot.overrun);
        assert!(snapshot.queues.is_none());
        assert_eq!(snapshot.samples.len(), 2);
        assert_eq!(
            snapshot.samples[0].wall_start_unix_ns,
            times.wall_start_unix_ns
        );
        assert_eq!(snapshot.samples[0].wall_us, 99);
        assert_eq!(snapshot.samples[0].total_us, 45);
        assert_eq!(snapshot.samples[0].phase_us.entity_us, 11);
        assert_eq!(snapshot.samples[0].phase_us.merge_us, 12);
        assert_eq!(snapshot.samples[0].phase_us.block_us, 13);
        assert_eq!(snapshot.samples[0].phase_us.snapshot_us, 14);
        assert_eq!(snapshot.samples[0].phase_us.views_us, 15);
        let phases = serde_json::to_value(snapshot.samples[0].phase_us).unwrap();
        for (name, expected) in [
            ("input_us", 16),
            ("chunks_us", 17),
            ("commit_us", 18),
            ("publish_us", 19),
            ("plugins_us", 20),
            ("coordination_us", 21),
            ("journal_build_us", 22),
            ("journal_queue_us", 23),
            ("generated_us", 24),
            ("owner_pre_us", 25),
            ("owner_post_us", 26),
        ] {
            assert_eq!(phases[name], expected, "{name}");
        }
        assert_eq!(snapshot.samples[1].tick, 44);
        assert_eq!(
            snapshot.samples[1].wall_start_unix_ns,
            times.wall_start_unix_ns + 987_654_321
        );
        assert_eq!(state.snapshot().tick.samples, 0);
        assert_eq!(state.load_snapshot(Some(44)).unwrap().samples.len(), 0);
    }

    #[test]
    fn load_phases_include_the_complete_tick_envelope() {
        let state = test_state();
        state.record_load_tick(1, TickTimes::default());
        let snapshot = state.load_snapshot(None).unwrap();
        let value = serde_json::to_value(snapshot.samples[0]).unwrap();
        for phase in [
            "input_us",
            "chunks_us",
            "commit_us",
            "publish_us",
            "plugins_us",
            "coordination_us",
            "journal_build_us",
            "journal_queue_us",
            "generated_us",
            "owner_pre_us",
            "owner_post_us",
        ] {
            assert!(value["phase_us"][phase].is_u64(), "missing phase {phase}");
        }
    }

    #[test]
    fn load_ring_is_bounded_and_reports_only_requested_overwrite() {
        let state = test_state();
        for tick in 1..=LOAD_SAMPLE_CAPACITY as u64 + 2 {
            state.record_load_tick(tick, TickTimes::default());
        }
        let snapshot = state.load_snapshot(Some(1)).unwrap();
        assert_eq!(snapshot.capacity, LOAD_SAMPLE_CAPACITY);
        assert_eq!(snapshot.samples.len(), LOAD_SAMPLE_CAPACITY);
        assert_eq!(snapshot.oldest_tick, Some(3));
        assert_eq!(snapshot.latest_tick, Some(LOAD_SAMPLE_CAPACITY as u64 + 2));
        assert_eq!(snapshot.overwritten_samples_total, 2);
        assert!(snapshot.overrun);
        assert!(!state.load_snapshot(Some(2)).unwrap().overrun);
        assert!(!state.load_snapshot(None).unwrap().overrun);
    }

    #[test]
    fn load_query_is_strict_and_unsigned() {
        let default = LoadMetricsQuery {
            after_tick: None,
            wait: false,
            min_samples: 1,
        };
        assert_eq!(parse_load_query(None), Ok(default));
        assert_eq!(parse_load_query(Some("")).unwrap().after_tick, None);
        assert_eq!(
            parse_load_query(Some("after_tick=0")).unwrap().after_tick,
            Some(0)
        );
        assert_eq!(
            parse_load_query(Some("after_tick=18446744073709551615"))
                .unwrap()
                .after_tick,
            Some(u64::MAX)
        );
        assert_eq!(
            parse_load_query(Some("after_tick=42&wait=true&min_samples=20")),
            Ok(LoadMetricsQuery {
                after_tick: Some(42),
                wait: true,
                min_samples: 20
            })
        );
        assert_eq!(
            parse_load_query(Some("wait=false&min_samples=1200"))
                .unwrap()
                .min_samples,
            1200
        );
        for query in [
            "after_tick=",
            "after_tick=-1",
            "after_tick=+1",
            "after_tick=18446744073709551616",
            "after_tick=1&after_tick=2",
            "after_tick=1&unknown=2",
            "after_tick",
            "wait=1",
            "wait=true&wait=false",
            "min_samples=0",
            "min_samples=1201",
            "min_samples=-1",
            "min_samples=1&min_samples=2",
        ] {
            assert_eq!(parse_load_query(Some(query)), Err(()), "{query}");
        }
    }

    #[test]
    fn load_wait_is_push_driven_and_notifies_every_reader_after_batch() {
        use std::task::{Context, Poll, Waker};

        let state = test_state();
        let mut first = std::pin::pin!(state.wait_for_load_samples(Some(40), 2));
        let mut second = std::pin::pin!(state.wait_for_load_samples(Some(40), 2));
        let mut context = Context::from_waker(Waker::noop());
        assert_eq!(first.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(second.as_mut().poll(&mut context), Poll::Pending);
        state.record_load_tick(41, TickTimes::default());
        assert_eq!(first.as_mut().poll(&mut context), Poll::Pending);
        assert_eq!(second.as_mut().poll(&mut context), Poll::Pending);
        state.record_load_tick(42, TickTimes::default());
        assert_eq!(first.as_mut().poll(&mut context), Poll::Ready(Some(())));
        assert_eq!(second.as_mut().poll(&mut context), Poll::Ready(Some(())));
        let mut already_ready = std::pin::pin!(state.wait_for_load_samples(Some(40), 2));
        assert_eq!(
            already_ready.as_mut().poll(&mut context),
            Poll::Ready(Some(()))
        );
    }

    #[tokio::test]
    async fn load_route_streams_raw_json_and_rejects_bad_cursor() {
        let state = test_state();
        let connections = Arc::new(ConnectionRegistry::new());
        state.set_queue_source(connections);
        state.record_load_tick(
            7,
            TickTimes {
                wall_start_unix_ns: 1_800_000_000_123_456_789,
                wall_us: 123,
                total_us: 45,
                ..Default::default()
            },
        );
        for (target, expected_status) in [
            ("/load-metrics?after_tick=6", "200 OK"),
            (
                "/load-metrics?after_tick=6&wait=true&min_samples=1",
                "200 OK",
            ),
            ("/load-metrics?after_tick=-1", "400 Bad Request"),
        ] {
            let (mut client, mut server) = duplex(8192);
            client
                .write_all(format!("GET {target} HTTP/1.1\r\nHost: x\r\n\r\n").as_bytes())
                .await
                .unwrap();
            serve_connection(&mut server, &state).await;
            drop(server);
            let mut out = Vec::new();
            client.read_to_end(&mut out).await.unwrap();
            let text = String::from_utf8(out).unwrap();
            assert!(text.starts_with(&format!("HTTP/1.1 {expected_status}")));
            if expected_status == "200 OK" {
                let body = text.split_once("\r\n\r\n").unwrap().1;
                let value: serde_json::Value = serde_json::from_str(body).unwrap();
                assert_eq!(value["schema_version"], 1);
                assert_eq!(value["wait_timed_out"], false);
                assert_eq!(value["samples"][0]["tick"], 7);
                assert_eq!(value["samples"][0]["wall_us"], 123);
                assert_eq!(
                    value["samples"][0]["wall_start_unix_ns"].as_u64(),
                    Some(1_800_000_000_123_456_789)
                );
                assert_eq!(value["queues"]["capabilities"]["queued_bytes"], false);
                assert!(value["queues"]["reliable"]["queued_bytes"].is_null());
                assert!(value["queues"]["world"]["reserved_slots"].is_null());
            }
        }
    }

    #[test]
    fn tick_percentiles_report_core_cost_and_publish_time() {
        let state = test_state();
        // Core cost 1..=100 ms, publish 1/10 of it; arrival gaps are ~0 here,
        // so any percentile that leaked from the arrival window would show.
        for ms in 1..=100u64 {
            state.record_pump_tick(PumpTick {
                tick: ms,
                core_us: ms * 1000,
                publish_us: ms * 100,
                ..Default::default()
            });
        }
        let snap = state.snapshot();
        assert_eq!(snap.simulation_tick, 100);
        assert_eq!(snap.tick.samples, 100);
        assert_eq!(snap.tick.p50_us, 50_000);
        assert_eq!(snap.tick.p95_us, 95_000);
        assert_eq!(snap.tick.max_us, 100_000);
        assert_eq!(snap.publish.p99_us, 9_900);
        assert!(!snap.autoscale);
        assert!(snap.warnings.is_empty());
    }

    #[test]
    fn tps_counts_ticks_the_pump_skipped() {
        let at = Instant::now();
        let second = std::time::Duration::from_secs(1);
        // The pump saw only ticks 0 and 20 one second apart (19 skipped):
        // the core still ran at 20 TPS.
        let arrivals = VecDeque::from([(0, at), (20, at + second)]);
        assert!((tps_of(&arrivals) - 20.0).abs() < 1e-6);
        let slow = VecDeque::from([(0, at), (5, at + second)]);
        assert!((tps_of(&slow) - 5.0).abs() < 1e-6);
        assert_eq!(tps_of(&VecDeque::from([(7, at)])), 0.0);
    }

    #[test]
    fn parse_head_preserves_query_for_load_route() {
        let req = parse_head("GET /stats?x=1 HTTP/1.1\r\nHost: a\r\n\r\n").unwrap();
        assert_eq!(req.method, "GET");
        assert_eq!(req.path, "/stats");
        assert_eq!(req.query.as_deref(), Some("x=1"));
    }

    #[tokio::test]
    async fn stats_route_serves_json() {
        let state = test_state();
        let (mut client, mut server) = duplex(8192);
        client
            .write_all(b"GET /stats HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        serve_connection(&mut server, &state).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("\"simulation_tick\""));
    }

    #[tokio::test]
    async fn unknown_route_is_404() {
        let state = test_state();
        let (mut client, mut server) = duplex(8192);
        client
            .write_all(b"GET /nope HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        serve_connection(&mut server, &state).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.starts_with("HTTP/1.1 404 Not Found"), "{text}");
    }

    #[tokio::test]
    async fn profile_route_carries_schema_marker() {
        let state = test_state();
        let (mut client, mut server) = duplex(8192);
        client
            .write_all(b"GET /profile HTTP/1.1\r\nHost: x\r\n\r\n")
            .await
            .unwrap();
        serve_connection(&mut server, &state).await;
        drop(server);
        let mut out = Vec::new();
        client.read_to_end(&mut out).await.unwrap();
        let text = String::from_utf8_lossy(&out).into_owned();
        assert!(text.starts_with("HTTP/1.1 200 OK"), "{text}");
        assert!(text.contains("\"profile_schema\":1"));
        assert!(text.contains("\"captured_unix_ms\""));
    }
}
