//! `solaris:screen` relay state machine (ADR 0018 "Screen sharing"). Pure: no
//! I/O and no clock; callers pass `Instant`s and deliver the returned effects.
//! The server never decodes frame bytes: it only reassembles owner parts,
//! bounds them and forwards them to permitted viewers that are looking.
use std::{
    collections::{HashMap, HashSet},
    time::{Duration, Instant},
};

use bytes::Bytes;
use mc_core::boundary::{ConnectionId, EntityId};
use uuid::Uuid;

pub(crate) const CHANNEL: &str = "solaris:screen";
const VERSION: u8 = 1;

const MAX_WIDTH_BLOCKS: u8 = 16;
const MAX_SCREENS_PER_OWNER: usize = 4;
/// Clients keep at most this many remote screens; the server never has more open.
const MAX_SCREENS_TOTAL: usize = 128;
const OPEN_REACH: f64 = 8.0;
const VIEW_DISTANCE: f64 = 32.0;
const FOV_HALF_DEGREES: f64 = 35.0;
const EYE_HEIGHT: f64 = 1.62;

const DEMAND_WIDTH: u16 = 640;
const DEMAND_FPS: u8 = 5;
const MAX_PART_BYTES: usize = 30_000;
const MAX_PARTS: u8 = 5;
const MAX_FRAME_BYTES: usize = 131_072;
const MAX_FRAME_HEIGHT: u16 = 640;
const BYTES_PER_SECOND: f64 = 1_048_576.0;
const BYTES_BURST: f64 = 262_144.0;
/// Frames still in flight when DEMAND turns off are dropped, not punished.
const MAX_STRIKES: u32 = 30;
const MAX_PENDING_PER_OWNER: usize = 8;
const REQUEST_INTERVAL: Duration = Duration::from_secs(2);

mod server_kind {
    pub const DEMAND: u8 = 0;
    pub const FRAME: u8 = 1;
    pub const STOP: u8 = 2;
    pub const REFUSED: u8 = 3;
    pub const REQUESTED: u8 = 4;
    pub const PERMISSION: u8 = 5;
    pub const ACTIVE: u8 = 6;
}
/// `STOP` reasons.
pub(crate) mod stop {
    pub const LOOK_AWAY: u8 = 0;
    pub const OWNER_CLOSED: u8 = 1;
    pub const REVOKED: u8 = 2;
    pub const OWNER_LEFT: u8 = 3;
}
/// `REFUSED` reasons.
pub(crate) mod refused {
    /// Reason 0 (not allowed) is reserved; non-native clients get no relay at all.
    pub const NOT_MONITOR: u8 = 1;
    pub const OUT_OF_REACH: u8 = 2;
    pub const TOO_MANY: u8 = 3;
    pub const TAKEN: u8 = 4;
    pub const BAD_FRAME: u8 = 5;
    pub const NO_SCREEN: u8 = 6;
    pub const REPEAT: u8 = 7;
}

/// A monitor row: `width` blocks from the min-coordinate anchor along +X
/// (north/south faces) or +Z (west/east faces), one block high.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ScreenId {
    pub pos: [i32; 3],
    /// Direction id of the face carrying the screen: 2 north, 3 south, 4 west, 5 east.
    pub side: u8,
    pub width: u8,
    /// `Dimension::ALL` index.
    pub dimension: u8,
}

impl ScreenId {
    fn parse(reader: &mut Reader) -> Option<Self> {
        let id = Self {
            pos: [reader.i32()?, reader.i32()?, reader.i32()?],
            side: reader.u8()?,
            width: reader.u8()?,
            dimension: reader.u8()?,
        };
        ((2..=5).contains(&id.side)
            && (1..=MAX_WIDTH_BLOCKS).contains(&id.width)
            && id.dimension < 3)
            .then_some(id)
    }
    fn write(&self, out: &mut Vec<u8>) {
        for value in self.pos {
            out.extend(value.to_be_bytes());
        }
        out.extend([self.side, self.width, self.dimension]);
    }
    fn along_x(&self) -> bool {
        self.side < 4
    }
    pub(crate) fn blocks(&self) -> impl Iterator<Item = [i32; 3]> + '_ {
        (0..i32::from(self.width)).map(|step| {
            let [x, y, z] = self.pos;
            if self.along_x() {
                [x + step, y, z]
            } else {
                [x, y, z + step]
            }
        })
    }
    /// Face centre and outward normal.
    fn face(&self) -> ([f64; 3], [f64; 3]) {
        let [x, y, z] = self.pos.map(f64::from);
        let half = f64::from(self.width) / 2.0;
        match self.side {
            2 => ([x + half, y + 0.5, z], [0.0, 0.0, -1.0]),
            3 => ([x + half, y + 0.5, z + 1.0], [0.0, 0.0, 1.0]),
            4 => ([x, y + 0.5, z + half], [-1.0, 0.0, 0.0]),
            _ => ([x + 1.0, y + 0.5, z + half], [1.0, 0.0, 0.0]),
        }
    }
    /// Shortest distance from `eye` to the row's box.
    fn distance(&self, eye: [f64; 3]) -> f64 {
        let min = self.pos.map(f64::from);
        let extent = if self.along_x() {
            [f64::from(self.width), 1.0, 1.0]
        } else {
            [1.0, 1.0, f64::from(self.width)]
        };
        (0..3)
            .map(|axis| {
                let outside = (min[axis] - eye[axis]).max(eye[axis] - (min[axis] + extent[axis]));
                outside.max(0.0).powi(2)
            })
            .sum::<f64>()
            .sqrt()
    }
}

/// Published native pose of one player (eye position, not feet).
#[derive(Debug, Clone, Copy)]
pub(crate) struct Pose {
    pub dimension: u8,
    pub eye: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}
impl Pose {
    pub(crate) fn from_feet(dimension: u8, feet: [f64; 3], yaw: f32, pitch: f32) -> Self {
        Self {
            dimension,
            eye: [feet[0], feet[1] + EYE_HEIGHT, feet[2]],
            yaw,
            pitch,
        }
    }
    fn direction(&self) -> [f64; 3] {
        let (yaw, pitch) = (
            f64::from(self.yaw).to_radians(),
            f64::from(self.pitch).to_radians(),
        );
        [
            -yaw.sin() * pitch.cos(),
            -pitch.sin(),
            yaw.cos() * pitch.cos(),
        ]
    }
}

/// Does a player at `pose` look at the front of the row? In range, on the
/// front side of the face plane, and the view direction within the field of
/// view of the row (widened by the row's own angular size). No occlusion in v1.
pub(crate) fn looks_at(pose: &Pose, screen: &ScreenId) -> bool {
    if pose.dimension != screen.dimension {
        return false;
    }
    let (center, normal) = screen.face();
    let to_eye: [f64; 3] = std::array::from_fn(|axis| pose.eye[axis] - center[axis]);
    if dot(to_eye, normal) <= 0.0 || screen.distance(pose.eye) > VIEW_DISTANCE {
        return false;
    }
    let to_center = to_eye.map(|value| -value);
    let distance = dot(to_center, to_center).sqrt();
    let radius = (f64::from(screen.width).powi(2) / 4.0 + 0.25).sqrt();
    let widen = (radius / distance.max(radius)).asin();
    let cos = dot(pose.direction(), to_center) / distance.max(f64::EPSILON);
    cos.clamp(-1.0, 1.0).acos() <= FOV_HALF_DEGREES.to_radians() + widen
}
fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

struct Reader<'a>(&'a [u8]);
impl<'a> Reader<'a> {
    fn take(&mut self, count: usize) -> Option<&'a [u8]> {
        let (head, tail) = self.0.split_at_checked(count)?;
        self.0 = tail;
        Some(head)
    }
    fn u8(&mut self) -> Option<u8> {
        Some(self.take(1)?[0])
    }
    fn u16(&mut self) -> Option<u16> {
        Some(u16::from_be_bytes(self.take(2)?.try_into().ok()?))
    }
    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
    fn i32(&mut self) -> Option<i32> {
        Some(i32::from_be_bytes(self.take(4)?.try_into().ok()?))
    }
}

/// Client -> server message.
#[derive(Debug, PartialEq)]
pub(crate) enum Message<'a> {
    Open(ScreenId),
    Close(ScreenId),
    Part {
        id: ScreenId,
        seq: u32,
        width: u16,
        height: u16,
        codec: u8,
        index: u8,
        count: u8,
        data: &'a [u8],
    },
    Request(ScreenId),
    Answer {
        request: u32,
        allow: bool,
    },
    Revoke {
        id: ScreenId,
        viewer: Uuid,
    },
    Leave(ScreenId),
}

pub(crate) fn decode(bytes: &[u8]) -> Option<Message<'_>> {
    let mut reader = Reader(bytes);
    if reader.u8()? != VERSION {
        return None;
    }
    let kind = reader.u8()?;
    let message = match kind {
        0 => Message::Open(ScreenId::parse(&mut reader)?),
        1 => Message::Close(ScreenId::parse(&mut reader)?),
        2 => {
            let id = ScreenId::parse(&mut reader)?;
            Message::Part {
                id,
                seq: reader.u32()?,
                width: reader.u16()?,
                height: reader.u16()?,
                codec: reader.u8()?,
                index: reader.u8()?,
                count: reader.u8()?,
                data: std::mem::take(&mut reader.0),
            }
        }
        3 => Message::Request(ScreenId::parse(&mut reader)?),
        4 => Message::Answer {
            request: reader.u32()?,
            allow: match reader.u8()? {
                0 => false,
                1 => true,
                _ => return None,
            },
        },
        5 => Message::Revoke {
            id: ScreenId::parse(&mut reader)?,
            viewer: Uuid::from_slice(reader.take(16)?).ok()?,
        },
        6 => Message::Leave(ScreenId::parse(&mut reader)?),
        _ => return None,
    };
    reader.0.is_empty().then_some(message)
}

fn message(kind: u8, id: &ScreenId, body: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(17 + body.len());
    out.extend([VERSION, kind]);
    id.write(&mut out);
    out.extend(body);
    Bytes::from(out)
}
fn demand(id: &ScreenId, on: bool) -> Bytes {
    let mut body = vec![u8::from(on)];
    body.extend(DEMAND_WIDTH.to_be_bytes());
    body.push(DEMAND_FPS);
    message(server_kind::DEMAND, id, &body)
}
fn frame(id: &ScreenId, seq: u32, width: u16, height: u16, codec: u8, data: &[u8]) -> Bytes {
    let mut body = Vec::with_capacity(9 + data.len());
    body.extend(seq.to_be_bytes());
    body.extend(width.to_be_bytes());
    body.extend(height.to_be_bytes());
    body.push(codec);
    body.extend(data);
    message(server_kind::FRAME, id, &body)
}

/// One server -> client payload to deliver.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Effect {
    pub to: ConnectionId,
    pub payload: Bytes,
}

/// Block lookup supplied from the published native world: is `pos` a monitor
/// block whose `facing` is `side` (direction id 2 north, 3 south, 4 west, 5 east)?
pub(crate) trait World {
    fn is_monitor(&self, dimension: u8, pos: [i32; 3], side: u8) -> bool;
}

struct Player {
    entity: EntityId,
    uuid: Uuid,
    name: String,
    /// Dimension the client was last told the active screens of.
    known_dimension: Option<u8>,
    last_request: Option<Instant>,
}

struct Partial {
    seq: u32,
    width: u16,
    height: u16,
    count: u8,
    next: u8,
    data: Vec<u8>,
}

struct Open {
    owner: ConnectionId,
    demand: bool,
    partial: Option<Partial>,
    strikes: u32,
    budget: f64,
    updated: Instant,
}

struct Pending {
    owner: ConnectionId,
    viewer: ConnectionId,
    id: ScreenId,
}

#[derive(Default)]
pub(crate) struct Relay {
    players: HashMap<ConnectionId, Player>,
    poses: HashMap<ConnectionId, Pose>,
    screens: HashMap<ScreenId, Open>,
    /// `(owner, viewer)`: the owner allowed the viewer to see their screens.
    grants: HashSet<(ConnectionId, ConnectionId)>,
    /// `(viewer, screen)`: granted viewers currently looking at the screen.
    watching: HashSet<(ConnectionId, ScreenId)>,
    requests: HashMap<u32, Pending>,
    next_request: u32,
}

impl Relay {
    pub(crate) fn register(
        &mut self,
        id: ConnectionId,
        entity: EntityId,
        uuid: Uuid,
        name: String,
    ) {
        self.players.insert(
            id,
            Player {
                entity,
                uuid,
                name,
                known_dimension: None,
                last_request: None,
            },
        );
    }

    /// Connections and the entity whose pose `tick` needs.
    pub(crate) fn entities(&self) -> Vec<(ConnectionId, EntityId)> {
        self.players
            .iter()
            .map(|(id, player)| (*id, player.entity))
            .collect()
    }

    pub(crate) fn unregister(&mut self, id: ConnectionId) -> Vec<Effect> {
        let mut effects = Vec::new();
        for screen in self.owned(id) {
            self.close(&screen, None, stop::OWNER_LEFT, &mut effects);
        }
        self.grants.retain(|(_, viewer)| *viewer != id);
        self.requests
            .retain(|_, pending| pending.viewer != id && pending.owner != id);
        let gone: Vec<ScreenId> = self
            .watching
            .iter()
            .filter(|(viewer, _)| *viewer == id)
            .map(|(_, screen)| *screen)
            .collect();
        self.watching.retain(|(viewer, _)| *viewer != id);
        for screen in gone {
            self.sync_demand(&screen, &mut effects);
        }
        self.players.remove(&id);
        self.poses.remove(&id);
        effects
    }

    fn owned(&self, owner: ConnectionId) -> Vec<ScreenId> {
        let mut owned: Vec<ScreenId> = self
            .screens
            .iter()
            .filter(|(_, open)| open.owner == owner)
            .map(|(screen, _)| *screen)
            .collect();
        owned.sort_by_key(|screen| (screen.pos, screen.side));
        owned
    }

    fn sync_demand(&mut self, id: &ScreenId, effects: &mut Vec<Effect>) {
        let on = self.watching.iter().any(|(_, screen)| screen == id);
        if let Some(open) = self.screens.get_mut(id)
            && open.demand != on
        {
            open.demand = on;
            open.partial = None;
            open.strikes = 0;
            effects.push(Effect {
                to: open.owner,
                payload: demand(id, on),
            });
        }
    }

    /// Remove a screen: viewers granted by its owner are told to stop, every
    /// native client in the dimension learns it went idle, and when it was the
    /// owner's last screen the owner's grants and pending requests end too.
    fn close(&mut self, id: &ScreenId, refusal: Option<u8>, reason: u8, effects: &mut Vec<Effect>) {
        let Some(open) = self.screens.remove(id) else {
            return;
        };
        if let Some(refusal) = refusal {
            effects.push(Effect {
                to: open.owner,
                payload: message(server_kind::REFUSED, id, &[refusal]),
            });
        }
        let mut viewers: Vec<ConnectionId> = self
            .grants
            .iter()
            .filter(|(owner, _)| *owner == open.owner)
            .map(|(_, viewer)| *viewer)
            .collect();
        viewers.sort();
        for viewer in viewers {
            effects.push(Effect {
                to: viewer,
                payload: message(server_kind::STOP, id, &[reason]),
            });
        }
        self.watching.retain(|(_, screen)| screen != id);
        self.broadcast_active(id, false, open.owner, effects);
        if self.owned(open.owner).is_empty() {
            self.grants.retain(|(owner, _)| *owner != open.owner);
            self.requests
                .retain(|_, pending| pending.owner != open.owner);
        }
    }

    fn broadcast_active(
        &self,
        id: &ScreenId,
        on: bool,
        except: ConnectionId,
        effects: &mut Vec<Effect>,
    ) {
        let mut targets: Vec<ConnectionId> = self
            .players
            .iter()
            .filter(|(conn, player)| {
                **conn != except && player.known_dimension == Some(id.dimension)
            })
            .map(|(conn, _)| *conn)
            .collect();
        targets.sort();
        for to in targets {
            effects.push(Effect {
                to,
                payload: message(server_kind::ACTIVE, id, &[u8::from(on)]),
            });
        }
    }

    fn refuse(to: ConnectionId, id: &ScreenId, reason: u8) -> Vec<Effect> {
        vec![Effect {
            to,
            payload: message(server_kind::REFUSED, id, &[reason]),
        }]
    }

    pub(crate) fn handle(
        &mut self,
        from: ConnectionId,
        msg: Message<'_>,
        world: &dyn World,
        now: Instant,
    ) -> Vec<Effect> {
        if !self.players.contains_key(&from) {
            return Vec::new();
        }
        let mut effects = Vec::new();
        match msg {
            Message::Open(id) => return self.open(from, id, world, now),
            Message::Close(id) => {
                if self.screens.get(&id).is_some_and(|open| open.owner == from) {
                    self.close(&id, None, stop::OWNER_CLOSED, &mut effects);
                }
            }
            Message::Part {
                id,
                seq,
                width,
                height,
                codec,
                index,
                count,
                data,
            } => {
                return self.part(
                    from,
                    id,
                    (seq, width, height, codec, index, count),
                    data,
                    now,
                );
            }
            Message::Request(id) => return self.request(from, id, now),
            Message::Answer { request, allow } => {
                if self
                    .requests
                    .get(&request)
                    .is_some_and(|pending| pending.owner == from)
                    && let Some(pending) = self.requests.remove(&request)
                {
                    if allow {
                        self.grants.insert((from, pending.viewer));
                    }
                    effects.push(Effect {
                        to: pending.viewer,
                        payload: message_permission(&pending.id, allow),
                    });
                }
            }
            Message::Revoke { id, viewer } => {
                if self.screens.get(&id).is_some_and(|open| open.owner == from)
                    && let Some(target) = self
                        .players
                        .iter()
                        .find_map(|(conn, player)| (player.uuid == viewer).then_some(*conn))
                    && self.grants.remove(&(from, target))
                {
                    for screen in self.owned(from) {
                        effects.push(Effect {
                            to: target,
                            payload: message(server_kind::STOP, &screen, &[stop::REVOKED]),
                        });
                        self.watching.remove(&(target, screen));
                        self.sync_demand(&screen, &mut effects);
                    }
                }
            }
            Message::Leave(id) => {
                if let Some(open) = self.screens.get(&id) {
                    let owner = open.owner;
                    if self.grants.remove(&(owner, from)) {
                        for screen in self.owned(owner) {
                            self.watching.remove(&(from, screen));
                            self.sync_demand(&screen, &mut effects);
                        }
                    }
                }
            }
        }
        effects
    }

    fn open(
        &mut self,
        owner: ConnectionId,
        id: ScreenId,
        world: &dyn World,
        now: Instant,
    ) -> Vec<Effect> {
        match self.screens.get(&id) {
            Some(open) if open.owner == owner => return Vec::new(),
            Some(_) => return Self::refuse(owner, &id, refused::TAKEN),
            None => {}
        }
        if self.owned(owner).len() >= MAX_SCREENS_PER_OWNER
            || self.screens.len() >= MAX_SCREENS_TOTAL
        {
            return Self::refuse(owner, &id, refused::TOO_MANY);
        }
        let Some(pose) = self.poses.get(&owner) else {
            return Self::refuse(owner, &id, refused::OUT_OF_REACH);
        };
        if pose.dimension != id.dimension || id.distance(pose.eye) > OPEN_REACH {
            return Self::refuse(owner, &id, refused::OUT_OF_REACH);
        }
        if !id
            .blocks()
            .all(|pos| world.is_monitor(id.dimension, pos, id.side))
        {
            return Self::refuse(owner, &id, refused::NOT_MONITOR);
        }
        self.screens.insert(
            id,
            Open {
                owner,
                demand: false,
                partial: None,
                strikes: 0,
                budget: BYTES_BURST,
                updated: now,
            },
        );
        let mut effects = vec![Effect {
            to: owner,
            payload: demand(&id, false),
        }];
        self.broadcast_active(&id, true, owner, &mut effects);
        effects
    }

    fn request(&mut self, viewer: ConnectionId, id: ScreenId, now: Instant) -> Vec<Effect> {
        let Some(open) = self.screens.get(&id) else {
            return Self::refuse(viewer, &id, refused::NO_SCREEN);
        };
        let owner = open.owner;
        let busy = owner == viewer
            || self.grants.contains(&(owner, viewer))
            || self
                .requests
                .values()
                .any(|p| p.owner == owner && p.viewer == viewer)
            || self.requests.values().filter(|p| p.owner == owner).count() >= MAX_PENDING_PER_OWNER
            || self
                .players
                .get(&viewer)
                .and_then(|p| p.last_request)
                .is_some_and(|last| now.saturating_duration_since(last) < REQUEST_INTERVAL);
        if busy {
            return Self::refuse(viewer, &id, refused::REPEAT);
        }
        let Some(player) = self.players.get_mut(&viewer) else {
            return Vec::new();
        };
        player.last_request = Some(now);
        let (uuid, name) = (player.uuid, player.name.clone());
        self.next_request = self.next_request.wrapping_add(1);
        let request = self.next_request;
        self.requests.insert(request, Pending { owner, viewer, id });
        let mut body = request.to_be_bytes().to_vec();
        body.extend(uuid.as_bytes());
        let name = &name.as_bytes()[..name.len().min(255)];
        body.push(name.len() as u8);
        body.extend(name);
        vec![Effect {
            to: owner,
            payload: message(server_kind::REQUESTED, &id, &body),
        }]
    }

    fn part(
        &mut self,
        from: ConnectionId,
        id: ScreenId,
        (seq, width, height, codec, index, count): (u32, u16, u16, u8, u8, u8),
        data: &[u8],
        now: Instant,
    ) -> Vec<Effect> {
        let Some(open) = self.screens.get_mut(&id).filter(|open| open.owner == from) else {
            return Self::refuse(from, &id, refused::NO_SCREEN);
        };
        let mut effects = Vec::new();
        if !open.demand {
            open.strikes += 1;
            if open.strikes > MAX_STRIKES {
                self.close(
                    &id,
                    Some(refused::BAD_FRAME),
                    stop::OWNER_CLOSED,
                    &mut effects,
                );
            }
            return effects;
        }
        let valid = codec == 0
            && (1..=DEMAND_WIDTH).contains(&width)
            && (1..=MAX_FRAME_HEIGHT).contains(&height)
            && (1..=MAX_PARTS).contains(&count)
            && index < count
            && !data.is_empty()
            && data.len() <= MAX_PART_BYTES;
        if !valid {
            self.close(
                &id,
                Some(refused::BAD_FRAME),
                stop::OWNER_CLOSED,
                &mut effects,
            );
            return effects;
        }
        if index == 0 {
            open.partial = Some(Partial {
                seq,
                width,
                height,
                count,
                next: 0,
                data: Vec::new(),
            });
        }
        let Some(partial) = open.partial.as_mut().filter(|partial| {
            partial.seq == seq
                && partial.next == index
                && partial.count == count
                && partial.width == width
                && partial.height == height
        }) else {
            open.partial = None;
            return effects;
        };
        partial.data.extend_from_slice(data);
        partial.next += 1;
        if partial.data.len() > MAX_FRAME_BYTES {
            self.close(
                &id,
                Some(refused::BAD_FRAME),
                stop::OWNER_CLOSED,
                &mut effects,
            );
            return effects;
        }
        if partial.next < partial.count {
            return effects;
        }
        let complete = open.partial.take().expect("partial exists");
        let elapsed = now.saturating_duration_since(open.updated).as_secs_f64();
        open.updated = now;
        open.budget = (open.budget + elapsed * BYTES_PER_SECOND).min(BYTES_BURST);
        if open.budget < complete.data.len() as f64 {
            return effects;
        }
        open.budget -= complete.data.len() as f64;
        let payload = frame(&id, seq, width, height, codec, &complete.data);
        let mut viewers: Vec<ConnectionId> = self
            .watching
            .iter()
            .filter(|(_, screen)| *screen == id)
            .map(|(viewer, _)| *viewer)
            .collect();
        viewers.sort();
        effects.extend(viewers.into_iter().map(|to| Effect {
            to,
            payload: payload.clone(),
        }));
        effects
    }

    /// Re-evaluate every screen against the newly published poses.
    pub(crate) fn tick(
        &mut self,
        poses: HashMap<ConnectionId, Pose>,
        world: &dyn World,
    ) -> Vec<Effect> {
        let mut effects = Vec::new();
        self.poses = poses;
        // Clients learn the active screens of the dimension they are in.
        let mut moved: Vec<(ConnectionId, u8)> = self
            .players
            .iter()
            .filter_map(|(conn, player)| {
                let pose = self.poses.get(conn)?;
                (player.known_dimension != Some(pose.dimension)).then_some((*conn, pose.dimension))
            })
            .collect();
        moved.sort();
        for (conn, dimension) in moved {
            self.players
                .get_mut(&conn)
                .expect("registered")
                .known_dimension = Some(dimension);
            let mut active: Vec<&ScreenId> = self
                .screens
                .iter()
                .filter(|(screen, open)| screen.dimension == dimension && open.owner != conn)
                .map(|(screen, _)| screen)
                .collect();
            active.sort_by_key(|screen| (screen.pos, screen.side));
            for screen in active {
                effects.push(Effect {
                    to: conn,
                    payload: message(server_kind::ACTIVE, screen, &[1]),
                });
            }
        }
        // A show ends on close/revoke/disconnect or when its monitors are broken
        // or unloaded; walking away never ends it (reach applies to OPEN only).
        let mut all: Vec<(ScreenId, ConnectionId)> = self
            .screens
            .iter()
            .map(|(screen, open)| (*screen, open.owner))
            .collect();
        all.sort_by_key(|(screen, owner)| (*owner, screen.pos, screen.side));
        for (screen, owner) in all {
            let failure = (!screen
                .blocks()
                .all(|pos| world.is_monitor(screen.dimension, pos, screen.side)))
            .then_some(refused::NOT_MONITOR);
            if let Some(reason) = failure {
                self.close(&screen, Some(reason), stop::OWNER_CLOSED, &mut effects);
                continue;
            }
            let mut grantees: Vec<ConnectionId> = self
                .grants
                .iter()
                .filter(|(grant_owner, _)| *grant_owner == owner)
                .map(|(_, viewer)| *viewer)
                .collect();
            grantees.sort();
            for viewer in grantees {
                let looking = self
                    .poses
                    .get(&viewer)
                    .is_some_and(|pose| looks_at(pose, &screen));
                let key = (viewer, screen);
                if looking {
                    self.watching.insert(key);
                } else if self.watching.remove(&key) {
                    effects.push(Effect {
                        to: viewer,
                        payload: message(server_kind::STOP, &screen, &[stop::LOOK_AWAY]),
                    });
                }
            }
            self.sync_demand(&screen, &mut effects);
        }
        effects
    }
}

fn message_permission(id: &ScreenId, allowed: bool) -> Bytes {
    message(server_kind::PERMISSION, id, &[u8::from(allowed)])
}

#[cfg(test)]
#[path = "screen_tests.rs"]
mod tests;
