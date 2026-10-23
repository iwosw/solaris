//! Edge glue for the `solaris:screen` relay: connection lifetime, native pose
//! publication and delivery. Policy lives in `screen.rs`; never on the tick.
use std::{collections::HashMap, sync::OnceLock, time::Instant};

use mc_core::boundary::{ConnectionId, EntityId, TickOutput};
use mc_core::dimensions::Dimension;
use mc_data::Identifier;
use mc_protocol::packets::{CustomPayload, Packet, play::ServerboundCustomPayload};
use uuid::Uuid;

use crate::{
    screen::{CHANNEL, Effect, Pose, Relay, World, decode},
    server::EdgeServer,
};

/// `solaris:monitor` is carried by this vanilla block (core `carrier_block_id` confirms);
/// its `facing` property is the face the screen is on.
const MONITOR_CARRIER: &str = "minecraft:black_glazed_terracotta";

#[derive(Default)]
pub(crate) struct Service {
    relay: parking_lot::Mutex<Relay>,
    latest: parking_lot::Mutex<Option<TickOutput>>,
    /// `(state id, side)` of every monitor carrier state.
    monitor_states: OnceLock<Vec<(u32, u8)>>,
}

struct Published<'a> {
    tick: Option<&'a TickOutput>,
    monitors: &'a [(u32, u8)],
}
impl World for Published<'_> {
    fn is_monitor(&self, dimension: u8, pos: [i32; 3], side: u8) -> bool {
        let Some(dimension) = Dimension::ALL.get(usize::from(dimension)) else {
            return false;
        };
        let Some(snapshot) = self.tick.and_then(|tick| tick.in_dimension(*dimension)) else {
            return false;
        };
        let snapshot = &snapshot.snapshot;
        let key = (pos[0].div_euclid(16), pos[2].div_euclid(16));
        let Ok(index) = snapshot
            .chunks
            .binary_search_by_key(&key, |chunk| (chunk.pos.x, chunk.pos.z))
        else {
            return false;
        };
        snapshot.chunks[index]
            .chunk
            .get_block(
                pos[0].rem_euclid(16) as u8,
                pos[1],
                pos[2].rem_euclid(16) as u8,
            )
            .is_some_and(|state| self.monitors.contains(&(state.0, side)))
    }
}

/// The payload of a `solaris:screen` Play custom payload, if it is one.
pub(crate) fn payload(body: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
    let mut body = body;
    let packet = ServerboundCustomPayload::decode(&mut body)?;
    if let CustomPayload::Unknown { channel, payload } = packet.payload
        && channel.as_str() == CHANNEL
    {
        return Ok(Some(payload));
    }
    Ok(None)
}

/// Normal connection framing (the negotiated compression threshold applies).
fn frame(payload: &[u8]) -> anyhow::Result<bytes::Bytes> {
    crate::lod::frame(Identifier::parse(CHANNEL)?, payload.to_vec())
}

/// Per-connection handle: dropping it ends the connection's screens/grants.
pub(crate) struct Connection {
    server: std::sync::Arc<EdgeServer>,
    id: ConnectionId,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.server.screens.unregister(&self.server, self.id);
    }
}
impl Connection {
    pub(crate) fn start(
        server: std::sync::Arc<EdgeServer>,
        id: ConnectionId,
        entity: EntityId,
        uuid: Uuid,
        name: String,
    ) -> Self {
        server.screens.relay.lock().register(id, entity, uuid, name);
        Self { server, id }
    }
    pub(crate) fn receive(&self, bytes: &[u8]) {
        self.server.screens.receive(&self.server, self.id, bytes);
    }
}

/// Native pose of a living player entity in the published tick.
pub(crate) fn published_pose(tick: &TickOutput, entity: EntityId) -> Option<Pose> {
    let (dimension, view) = std::iter::once(tick)
        .chain(tick.snapshot.dimensions.values())
        .find_map(|out| Some((out.snapshot.dimension, out.snapshot.entities.get(entity)?)))?;
    let index = Dimension::ALL.iter().position(|d| *d == dimension)?;
    (view.appearance.health > 0.0)
        .then(|| Pose::from_feet(index as u8, view.position, view.yaw, view.pitch))
}

impl Service {
    fn monitors(&self, server: &EdgeServer) -> &[(u32, u8)] {
        self.monitor_states.get_or_init(|| {
            let Some(block) = Identifier::parse(MONITOR_CARRIER)
                .ok()
                .and_then(|name| server.wire.blocks.block(&name))
            else {
                return Vec::new();
            };
            block
                .states
                .iter()
                .filter_map(|state| {
                    server
                        .hub
                        .registry()
                        .carrier_block_id(state.0)
                        .filter(|id| id.as_str() == "solaris:monitor")?;
                    let facing = server
                        .wire
                        .blocks
                        .by_id(*state)?
                        .properties
                        .iter()
                        .find(|(name, _)| name == "facing")?
                        .1
                        .as_str();
                    let side = match facing {
                        "north" => 2,
                        "south" => 3,
                        "west" => 4,
                        "east" => 5,
                        _ => return None,
                    };
                    Some((state.0, side))
                })
                .collect()
        })
    }

    fn receive(&self, server: &EdgeServer, id: ConnectionId, bytes: &[u8]) {
        let Some(message) = decode(bytes) else {
            tracing::warn!(connection = id.0, "screen message malformed; ignored");
            return;
        };
        let latest = self.latest.lock().clone();
        let world = Published {
            tick: latest.as_ref(),
            monitors: self.monitors(server),
        };
        // State change and enqueue share one critical section so DEMAND on/off
        // and STOP can never be reordered between concurrent callers.
        let mut relay = self.relay.lock();
        let effects = relay.handle(id, message, &world, Instant::now());
        deliver(server, &mut relay, effects);
    }

    fn unregister(&self, server: &EdgeServer, id: ConnectionId) {
        let mut relay = self.relay.lock();
        let effects = relay.unregister(id);
        deliver(server, &mut relay, effects);
    }

    pub(crate) fn tick(&self, server: &EdgeServer, tick: &TickOutput) {
        let entities = self.relay.lock().entities();
        let mut poses = HashMap::with_capacity(entities.len());
        for (connection, entity) in entities {
            if let Some(pose) = published_pose(tick, entity) {
                poses.insert(connection, pose);
            }
        }
        *self.latest.lock() = Some(tick.clone());
        let world = Published {
            tick: Some(tick),
            monitors: self.monitors(server),
        };
        let mut relay = self.relay.lock();
        let effects = relay.tick(poses, &world);
        deliver(server, &mut relay, effects);
    }
}

/// Delivery on the reliable queue, called with the relay lock held (only
/// non-blocking `try_send`). A full queue drops a FRAME or AUDIO packet (stale
/// media is worthless); control messages (DEMAND/STOP/ACTIVE/REQUESTED/PERMISSION/
/// REFUSED/KEYFRAME_REQUEST) follow the normal command-queue rule: a full queue
/// retires the connection. One encoding serves all viewers of the same relayed payload.
/// A dropped FRAME breaks that viewer's delta chain, so the relay withholds deltas from it
/// and asks the owner for a key frame (the resulting effects are delivered too).
fn deliver(server: &EdgeServer, relay: &mut Relay, effects: Vec<Effect>) {
    let mut last: Option<(*const u8, usize, bytes::Bytes)> = None;
    let mut queue: std::collections::VecDeque<Effect> = effects.into();
    while let Some(effect) = queue.pop_front() {
        let key = (effect.payload.as_ptr(), effect.payload.len());
        let framed = match &last {
            Some((ptr, len, framed)) if (*ptr, *len) == key => framed.clone(),
            _ => match frame(&effect.payload) {
                Ok(framed) => {
                    last = Some((key.0, key.1, framed.clone()));
                    framed
                }
                Err(error) => {
                    tracing::warn!(%error, "screen payload not encodable");
                    continue;
                }
            },
        };
        if let Some(handles) = server.connections.get(effect.to)
            && handles.reliable_tx.try_send(framed).is_err()
        {
            if !effect.is_media() {
                tracing::warn!(
                    connection = effect.to.0,
                    "screen control queue full; connection retired"
                );
                server.connections.retire(effect.to);
            } else if let Some(screen) = effect.video_screen() {
                queue.extend(relay.frame_dropped(effect.to, screen));
            }
        }
    }
}

pub(crate) fn spawn_tick_task(server: std::sync::Arc<EdgeServer>) -> tokio::task::JoinHandle<()> {
    let mut ticks = server.hub.subscribe();
    tokio::spawn(async move {
        loop {
            match ticks.recv().await {
                Ok(tick) => server.screens.tick(&server, &tick),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
