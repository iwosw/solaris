//! Per-connection DH negotiation, cancellation and low-priority transfers.
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::Instant;

use mc_core::boundary::{ConnectionId, EntityId, TickOutput};
use mc_core::dimensions::Dimension;
use tokio::sync::{OwnedSemaphorePermit, Semaphore, mpsc};
use tokio::task::{AbortHandle, JoinSet};

use super::{
    Service, Tile,
    budget::{Bucket, FRAGMENT, FRAME_BURST, PLAYER_BYTES, PLAYER_JOBS},
    codec::{self, Config, Message, Section},
};
use crate::server::EdgeServer;

pub(crate) struct Outbound {
    pub(crate) bytes: bytes::Bytes,
    valid: Arc<AtomicBool>,
    epoch: Arc<AtomicBool>,
    alive: Arc<AtomicBool>,
    _reservation: Option<Arc<OwnedSemaphorePermit>>,
}
impl Outbound {
    pub(crate) fn valid(&self) -> bool {
        self.valid.load(Ordering::Acquire)
            && self.epoch.load(Ordering::Acquire)
            && self.alive.load(Ordering::Acquire)
    }
}

enum Ingress {
    Payload(Vec<u8>),
    Respawn,
}
pub(crate) struct Connection {
    ingress: mpsc::Sender<Ingress>,
    task: AbortHandle,
    valid: Arc<AtomicBool>,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.valid.store(false, Ordering::Release);
        self.task.abort();
    }
}
impl Connection {
    pub(crate) fn start(
        server: Arc<EdgeServer>,
        id: ConnectionId,
        entity: EntityId,
        dimension: Dimension,
        outgoing: mpsc::Sender<Outbound>,
    ) -> Self {
        let (ingress, rx) = mpsc::channel(16);
        let valid = Arc::new(AtomicBool::new(true));
        let session = Session::new(server, id, entity, dimension, outgoing, Arc::clone(&valid));
        let task = tokio::spawn(session.run(rx)).abort_handle();
        Self {
            ingress,
            task,
            valid,
        }
    }
    pub(crate) fn receive(&self, bytes: Vec<u8>) {
        // A malformed/overloaded optional session never tears down vanilla Play.
        if bytes.len() > 65_536 || self.ingress.try_send(Ingress::Payload(bytes)).is_err() {
            self.valid.store(false, Ordering::Release);
            self.task.abort();
        }
    }
    pub(crate) fn respawn(&self) {
        if self.ingress.try_send(Ingress::Respawn).is_err() {
            self.valid.store(false, Ordering::Release);
            self.task.abort();
        }
    }
}

struct Job {
    abort: AbortHandle,
    valid: Arc<AtomicBool>,
}
struct Transfer {
    request: Option<i32>,
    buffer: i32,
    tile: Arc<Tile>,
    offset: usize,
    valid: Arc<AtomicBool>,
    reservation: Arc<OwnedSemaphorePermit>,
}
impl Transfer {
    fn message(&self, level: String, fragment: usize) -> (Message, usize) {
        let end = (self.offset + fragment).min(self.tile.bytes.len());
        let message = if self.offset < end {
            Message::Split {
                buffer: self.buffer,
                data: self.tile.bytes[self.offset..end].to_vec(),
                first: self.offset == 0,
            }
        } else if let Some(id) = self.request {
            Message::Response {
                id,
                buffer: Some(self.buffer),
            }
        } else {
            Message::Update {
                level,
                buffer: self.buffer,
            }
        };
        (message, end)
    }
}
type CompletedJob = (i64, i64, Arc<AtomicBool>, Result<Arc<Tile>, String>);

struct Session {
    server: Arc<EdgeServer>,
    service: Arc<Service>,
    id: ConnectionId,
    entity: EntityId,
    dimension: Dimension,
    anchor: Option<[f64; 2]>,
    border: Option<mc_core::command_border::Border>,
    offer: Option<Config>,
    level: Option<String>,
    jobs: HashMap<i64, Job>,
    tasks: JoinSet<CompletedJob>,
    transfers: VecDeque<Transfer>,
    dirty: HashSet<i64>,
    next_update: i64,
    tickets: HashMap<i32, Arc<AtomicBool>>,
    epoch: Arc<AtomicBool>,
    deferred: Vec<Message>,
    /// Alternate the shared slots when both request kinds are pending.
    last_update: bool,
    requests: Bucket,
    bandwidth: Bucket,
    pending: Arc<Semaphore>,
    outgoing: mpsc::Sender<Outbound>,
    valid: Arc<AtomicBool>,
    next_buffer: i32,
}
impl Session {
    fn new(
        server: Arc<EdgeServer>,
        id: ConnectionId,
        entity: EntityId,
        dimension: Dimension,
        outgoing: mpsc::Sender<Outbound>,
        valid: Arc<AtomicBool>,
    ) -> Self {
        let service = Arc::clone(server.dh.as_ref().expect("enabled DH"));
        Self {
            server,
            service,
            id,
            entity,
            dimension,
            anchor: None,
            border: None,
            offer: None,
            level: None,
            jobs: HashMap::new(),
            tasks: JoinSet::new(),
            transfers: VecDeque::new(),
            dirty: HashSet::new(),
            next_update: i64::from(i32::MAX) + 1,
            tickets: HashMap::new(),
            epoch: Arc::new(AtomicBool::new(true)),
            deferred: Vec::new(),
            last_update: false,
            requests: Bucket::new(2, PLAYER_JOBS),
            bandwidth: Bucket::new(500_000, FRAME_BURST),
            pending: Arc::new(Semaphore::new(PLAYER_BYTES)),
            outgoing,
            valid,
            next_buffer: 1,
        }
    }
    fn cancel_all(&mut self) {
        self.deferred.clear();
        self.last_update = false;
        self.epoch.store(false, Ordering::Release);
        self.epoch = Arc::new(AtomicBool::new(true));
        for (_, ticket) in self.tickets.drain() {
            ticket.store(false, Ordering::Release);
        }
        for (_, job) in self.jobs.drain() {
            job.valid.store(false, Ordering::Release);
            job.abort.abort();
        }
        for transfer in self.transfers.drain(..) {
            transfer.valid.store(false, Ordering::Release);
        }
        self.tasks.abort_all();
        self.dirty.clear();
    }
    fn config(&self, client: &Config) -> Config {
        let radius = i32::from(self.server.cfg.distant_horizons.radius_chunks);
        Config {
            plan: codec::PLAN_CHUNKS_ONLY,
            distance: radius,
            center_x: 0,
            center_z: 0,
            radius: 0,
            requests: 2,
            updates: true,
            update_radius: radius,
            sync: true,
            sync_distance: radius,
            sync_requests: 2,
            bandwidth: 500,
        }
        .constrain(client)
    }
    fn send(&self, message: Message) {
        if let Ok(bytes) = super::frame(&message) {
            let _ = self.outgoing.try_send(Outbound {
                bytes,
                valid: Arc::clone(&self.valid),
                epoch: Arc::clone(&self.epoch),
                alive: Arc::clone(&self.valid),
                _reservation: None,
            });
        }
    }
    fn error(&self, id: i32, kind: i32, message: &str) {
        self.send(Message::Exception {
            id,
            kind,
            message: message.into(),
        });
    }
    fn level_key(&self) -> String {
        format!("{}@{}", self.service.identity, self.dimension.name())
    }
    fn initialize(&mut self, dimension: String) {
        if dimension != self.dimension.name() {
            return;
        }
        let level = self.level_key();
        self.send(Message::LevelInit {
            dimension,
            server_key: self.service.identity.clone(),
            level_key: level.clone(),
            time: super::unix_ms(),
        });
        self.level = Some(level);
    }
    fn authorized(&self, section: Section, radius: i32) -> bool {
        let (Some(anchor), Some(border)) = (self.anchor, self.border) else {
            return false;
        };
        within_bounds(section, anchor, radius, border) && !self.server.hub.session_locked(self.id)
    }
    fn request(&mut self, id: i32, level: String, position: i64, timestamp: Option<i64>) {
        if self.deferred.len() >= PLAYER_JOBS
            || self.deferred.iter().any(
                |message| matches!(message, Message::Request { id: queued, .. } if *queued == id),
            )
        {
            self.error(id, codec::ERR_RATE_LIMITED, "DH request queue exhausted");
            return;
        }
        self.deferred.push(Message::Request {
            id,
            level,
            position,
            timestamp,
        });
    }
    fn dispatch_request(&mut self, id: i32, level: String, position: i64, timestamp: Option<i64>) {
        let Some(config) = self.offer.as_ref() else {
            self.error(id, codec::ERR_REJECTED, "DH configuration required");
            return;
        };
        if self.level.as_ref() != Some(&level) || level != self.level_key() {
            self.error(id, codec::ERR_REJECTED, "inactive level");
            return;
        }
        if (timestamp.is_none() && config.plan == codec::PLAN_DISABLED)
            || (timestamp.is_some() && !config.sync)
        {
            self.error(id, codec::ERR_REJECTED, "request mode disabled");
            return;
        }
        let Ok(section) = Section::decode(position) else {
            self.error(id, codec::ERR_OUT_OF_RANGE, "invalid section");
            return;
        };
        // Join authority is published on the next tick. Hold at most the same
        // two requests instead of trusting the client's initial coordinates.
        if self.anchor.is_none() {
            self.request(id, level, position, timestamp);
            return;
        }
        let radius = if timestamp.is_some() {
            config.sync_distance
        } else {
            config.distance
        };
        if !self.authorized(section, radius) {
            self.error(
                id,
                codec::ERR_OUT_OF_RANGE,
                "section outside authorized range",
            );
            return;
        }
        if section.detail < codec::LEAF_DETAIL {
            self.error(id, codec::ERR_REJECTED, "detail finer than a DH leaf");
            return;
        }
        if section.detail > codec::LEAF_DETAIL {
            self.error(id, codec::ERR_NEEDS_SPLIT, "request detail-6 leaves");
            return;
        }
        self.tickets
            .retain(|_, ticket| Arc::strong_count(ticket) > 1);
        let negotiated = if timestamp.is_some() {
            config.sync_requests
        } else {
            config.requests
        };
        if self.tickets.contains_key(&id) || negotiated <= 0 {
            self.error(id, codec::ERR_RATE_LIMITED, "DH request budget exhausted");
            return;
        }
        if (self.updates_pending() && !self.last_update)
            || self.jobs.len() + self.transfers.len() >= PLAYER_JOBS
            || self.tickets.len() >= (negotiated as usize).min(PLAYER_JOBS)
            || !self.requests.take(1, Instant::now())
        {
            self.request(id, level, position, timestamp);
            return;
        }
        self.last_update = false;
        self.launch(i64::from(id), position, timestamp);
    }
    fn launch(&mut self, id: i64, position: i64, timestamp: Option<i64>) {
        let service = Arc::clone(&self.service);
        let server = Arc::clone(&self.server);
        let dimension = self.dimension;
        let valid = Arc::new(AtomicBool::new(true));
        if let Ok(request) = i32::try_from(id) {
            self.tickets.insert(request, Arc::clone(&valid));
        }
        let ticket = Arc::clone(&valid);
        let abort = self.tasks.spawn(async move {
            let result = service.tile(server, dimension, position).await.map(|tile| {
                if timestamp.is_some_and(|time| time >= tile.modified) {
                    Arc::new(Tile {
                        bytes: Arc::new(Vec::new()),
                        modified: tile.modified,
                    })
                } else {
                    tile
                }
            });
            (id, position, ticket, result)
        });
        self.jobs.insert(id, Job { abort, valid });
    }
    fn cancel(&mut self, id: i32) {
        self.deferred.retain(
            |message| !matches!(message, Message::Request { id: queued, .. } if *queued == id),
        );
        if let Some(ticket) = self.tickets.remove(&id) {
            ticket.store(false, Ordering::Release);
        }
        if let Some(job) = self.jobs.remove(&i64::from(id)) {
            job.valid.store(false, Ordering::Release);
            job.abort.abort();
        }
        self.transfers.retain(|transfer| {
            let keep = transfer.request != Some(id);
            if !keep {
                transfer.valid.store(false, Ordering::Release);
            }
            keep
        });
    }
    fn publication(&mut self, output: &TickOutput) {
        let found = std::iter::once(output)
            .chain(output.snapshot.dimensions.values())
            .find_map(|out| {
                out.snapshot
                    .entities
                    .get(self.entity)
                    .map(|entity| (out.snapshot.dimension, entity.position, out))
            });
        let Some((dimension, position, output)) = found else {
            self.anchor = None;
            return;
        };
        if dimension != self.dimension {
            self.cancel_all();
            self.dimension = dimension;
            self.level = None;
            if self.offer.is_some() {
                self.initialize(dimension.name().into());
            }
        }
        self.anchor = Some([position[0], position[2]]);
        self.border = Some(output.snapshot.border);
        for block in &output.changes.blocks {
            let sx = block.position.x.div_euclid(64);
            let sz = block.position.z.div_euclid(64);
            // A warm client may never request its cached leaf in this session.
            // Updates subscribe to the negotiated area, not generation history.
            self.invalidate(([sx - 1, sz - 1], [sx + 1, sz + 1]));
        }
    }
    fn queue_result(
        &mut self,
        id: i64,
        position: i64,
        valid: Arc<AtomicBool>,
        result: Result<Arc<Tile>, String>,
    ) {
        self.jobs.remove(&id);
        if !valid.load(Ordering::Acquire) {
            return;
        }
        let request = i32::try_from(id).ok();
        let tile = match result {
            Ok(tile) => tile,
            Err(error) => {
                if let Some(id) = request {
                    self.error(id, codec::ERR_REJECTED, &error);
                } else if let Ok(section) = Section::decode(position) {
                    self.invalidate(([section.x, section.z], [section.x, section.z]));
                }
                return;
            }
        };
        let radius = self.offer.as_ref().map_or(0, |config| {
            if request.is_none() {
                config.update_radius
            } else {
                config.distance.max(config.sync_distance)
            }
        });
        if !Section::decode(position).is_ok_and(|section| self.authorized(section, radius)) {
            return;
        }
        if tile.bytes.is_empty() {
            if let Some(id) = request {
                self.send(Message::Response { id, buffer: None });
            }
            return;
        }
        let Ok(reservation) =
            Arc::clone(&self.pending).try_acquire_many_owned(tile.bytes.len() as u32)
        else {
            if let Some(id) = request {
                self.error(
                    id,
                    codec::ERR_RATE_LIMITED,
                    "DH pending-byte budget exhausted",
                );
            } else if let Ok(section) = Section::decode(position) {
                self.invalidate(([section.x, section.z], [section.x, section.z]));
            }
            return;
        };
        let buffer = self.next_buffer;
        self.next_buffer = self.next_buffer.wrapping_add(1);
        self.transfers.push_back(Transfer {
            request,
            buffer,
            tile,
            offset: 0,
            valid,
            reservation: Arc::new(reservation),
        });
    }
    fn flush(&mut self) {
        while let Some(transfer) = self.transfers.front() {
            if !transfer.valid.load(Ordering::Acquire) {
                self.transfers.pop_front();
                continue;
            }
            let fragment = self.offer.as_ref().map_or(FRAGMENT, |config| {
                (config.bandwidth.max(1) as usize * 1000).min(FRAGMENT)
            });
            let (message, end) = transfer.message(self.level_key(), fragment);
            let Ok(bytes) = super::frame(&message) else {
                self.transfers.pop_front();
                continue;
            };
            let size = bytes.len();
            let now = Instant::now();
            if !self.bandwidth.take(size, now) {
                break;
            }
            if !self
                .service
                .bandwidth
                .lock()
                .expect("DH bandwidth")
                .take(size, now)
            {
                self.bandwidth.refund(size);
                break;
            }
            let outgoing = Outbound {
                bytes,
                valid: Arc::clone(&transfer.valid),
                epoch: Arc::clone(&self.epoch),
                alive: Arc::clone(&self.valid),
                _reservation: Some(Arc::clone(&transfer.reservation)),
            };
            if self.outgoing.try_send(outgoing).is_err() {
                self.bandwidth.refund(size);
                self.service
                    .bandwidth
                    .lock()
                    .expect("DH bandwidth")
                    .refund(size);
                break;
            }
            if transfer.offset < end {
                self.transfers.front_mut().expect("front").offset = end;
            } else {
                self.transfers.pop_front();
            }
        }
    }
    fn invalidate(&mut self, bounds: ([i32; 2], [i32; 2])) {
        let (Some(config), Some(anchor), Some(border)) = (
            self.offer.as_ref().filter(|config| config.updates),
            self.anchor,
            self.border,
        ) else {
            return;
        };
        invalidate_area(
            &mut self.dirty,
            anchor,
            config.update_radius,
            border,
            bounds,
        );
    }
    fn invalidate_all(&mut self) {
        let (Some(config), Some(anchor), Some(border)) = (
            self.offer.as_ref().filter(|config| config.updates),
            self.anchor,
            self.border,
        ) else {
            return;
        };
        let radius = f64::from(config.update_radius) * 16.0;
        let min = anchor.map(|coordinate| ((coordinate - radius) / 64.0).floor() as i32);
        let max = anchor.map(|coordinate| ((coordinate + radius) / 64.0).floor() as i32);
        invalidate_area(
            &mut self.dirty,
            anchor,
            config.update_radius,
            border,
            (min, max),
        );
    }
    fn updates_pending(&self) -> bool {
        let Some(config) = self.offer.as_ref().filter(|config| config.updates) else {
            return false;
        };
        self.dirty.iter().any(|position| {
            Section::decode(*position)
                .is_ok_and(|section| self.authorized(section, config.update_radius))
        })
    }
    fn update(&mut self) {
        if self.level.is_none() {
            return;
        }
        let Some(config) = self.offer.as_ref().filter(|config| config.updates) else {
            return;
        };
        if self.jobs.len() + self.transfers.len() >= PLAYER_JOBS || self.dirty.is_empty() {
            return;
        }
        if self.last_update && !self.deferred.is_empty() {
            return;
        }
        let radius = config.update_radius;
        let Some(position) = self.dirty.iter().copied().find(|position| {
            Section::decode(*position).is_ok_and(|section| self.authorized(section, radius))
        }) else {
            return;
        };
        if !self.requests.take(1, Instant::now()) {
            return;
        }
        self.dirty.remove(&position);
        let id = self.next_update;
        self.next_update = self
            .next_update
            .checked_add(1)
            .unwrap_or(i64::from(i32::MAX) + 1);
        self.last_update = true;
        self.launch(id, position, None);
    }
    fn schedule(&mut self) {
        if self.anchor.is_some()
            && !self.deferred.is_empty()
            && let Message::Request {
                id,
                level,
                position,
                timestamp,
            } = self.deferred.remove(0)
        {
            self.dispatch_request(id, level, position, timestamp);
        }
        self.update();
    }
    async fn run(mut self, mut ingress: mpsc::Receiver<Ingress>) {
        let mut ticks = self.server.hub.subscribe();
        let mut close_reason = None;
        loop {
            tokio::select! {
                biased;
                message = ingress.recv() => {
                    let Some(message) = message else { break; };
                    let bytes = match message {
                        Ingress::Payload(bytes) => bytes,
                        Ingress::Respawn => { self.cancel_all(); self.anchor = None; continue; }
                    };
                    match codec::decode(&bytes) {
                        Ok(Message::Config(client)) => {
                            let reconfigure = self.offer.is_some();
                            if reconfigure { self.cancel_all(); }
                            let config = self.config(&client);
                            self.bandwidth = Bucket::new((config.bandwidth.max(1) as usize * 1000).min(500_000), FRAME_BURST);
                            self.offer = Some(config.clone()); self.send(Message::Config(config));
                            if reconfigure && self.level.is_some() { self.initialize(self.dimension.name().into()); }
                        }
                        Ok(Message::RequestLevelInit(dimension)) => self.initialize(dimension),
                        Ok(Message::Request { id, level, position, timestamp }) => self.request(id, level, position, timestamp),
                        Ok(Message::Cancel(id)) => self.cancel(id),
                        Ok(Message::Close(_)) => break,
                        Ok(_) => { close_reason = Some("unsupported client message direction".into()); break; }
                        Err(error) => { close_reason = Some(format!("DH protocol 16 required: {error}")); break; }
                    }
                }
                output = ticks.recv() => match output {
                    Ok(output) => self.publication(&output),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => self.invalidate_all(),
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                },
                result = self.tasks.join_next(), if !self.tasks.is_empty() => {
                    if let Some(Ok((id, position, valid, result))) = result { self.queue_result(id, position, valid, result); }
                }
            }
            self.flush();
            self.schedule();
        }
        self.cancel_all();
        if let Some(reason) = close_reason {
            self.send(Message::Close(reason));
        }
    }
}

fn within_bounds(
    section: Section,
    anchor: [f64; 2],
    radius: i32,
    border: mc_core::command_border::Border,
) -> bool {
    if radius < 0 || anchor.iter().any(|coordinate| !coordinate.is_finite()) {
        return false;
    }
    let Ok((min, max)) = section.bounds() else {
        return false;
    };
    let nearest = (0..2)
        .map(|axis| {
            (anchor[axis] - anchor[axis].clamp(f64::from(min[axis]), f64::from(max[axis]))).abs()
        })
        .fold(0.0, f64::max);
    let half = border.size / 2.0;
    nearest <= f64::from(radius) * 16.0
        && f64::from(min[0]) >= border.center_x - half
        && f64::from(max[0]) < border.center_x + half
        && f64::from(min[1]) >= border.center_z - half
        && f64::from(max[1]) < border.center_z + half
}

fn invalidate_area(
    dirty: &mut HashSet<i64>,
    anchor: [f64; 2],
    radius: i32,
    border: mc_core::command_border::Border,
    bounds: ([i32; 2], [i32; 2]),
) {
    for x in bounds.0[0]..=bounds.1[0] {
        for z in bounds.0[1]..=bounds.1[1] {
            let section = Section {
                detail: codec::LEAF_DETAIL,
                x,
                z,
            };
            if within_bounds(section, anchor, radius, border) {
                // Retain an explicit memory bound as the player moves. One
                // authorized 128-chunk area contains fewer than 8192 leaves.
                if dirty.len() >= 8192 {
                    dirty.retain(|position| {
                        Section::decode(*position)
                            .is_ok_and(|section| within_bounds(section, anchor, radius, border))
                    });
                }
                dirty.insert(section.packed());
            }
        }
    }
}

#[cfg(test)]
#[path = "session_tests.rs"]
mod tests;
