//! Edge glue for the `solaris:voice` relay: connection lifetime, native pose
//! publication and best-effort delivery. Policy lives in `voice.rs`.
use std::{collections::HashMap, sync::Arc, time::Instant};

use mc_core::boundary::{ConnectionId, EntityId, TickOutput};
use mc_data::Identifier;
use mc_protocol::packets::{CustomPayload, Packet, play::ServerboundCustomPayload};
use uuid::Uuid;

use crate::{
    server::EdgeServer,
    voice::{CHANNEL, Delivery, Relay, decode},
};

pub(crate) struct Service {
    relay: parking_lot::Mutex<Relay>,
}

impl Default for Service {
    fn default() -> Self {
        Self::new(&crate::voice::VoiceConfig::default())
    }
}

impl Service {
    pub(crate) fn new(config: &crate::voice::VoiceConfig) -> Self {
        Self {
            relay: parking_lot::Mutex::new(Relay::new(config)),
        }
    }

    fn tick(&self, tick: &TickOutput) {
        let entities = self.relay.lock().entities();
        let mut poses = HashMap::with_capacity(entities.len());
        for (connection, entity) in entities {
            if let Some(pose) = crate::screen_service::published_pose(tick, entity) {
                poses.insert(connection, pose);
            }
        }
        self.relay.lock().set_poses(poses);
    }
}

/// The payload of a `solaris:voice` Play custom payload, if it is one.
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

fn frame(payload: &[u8]) -> anyhow::Result<bytes::Bytes> {
    crate::lod::frame(Identifier::parse(CHANNEL)?, payload.to_vec())
}

/// Per-connection handle: dropping it unregisters the connection.
pub(crate) struct Connection {
    server: Arc<EdgeServer>,
    id: ConnectionId,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.server.voice.relay.lock().unregister(self.id);
    }
}
impl Connection {
    pub(crate) fn start(
        server: Arc<EdgeServer>,
        id: ConnectionId,
        entity: EntityId,
        uuid: Uuid,
    ) -> Self {
        let state = server
            .voice
            .relay
            .lock()
            .register(id, entity, uuid, Instant::now());
        if let Ok(framed) = frame(&state)
            && let Some(handles) = server.connections.get(id)
        {
            let _ = handles.reliable_tx.try_send(framed);
        }
        Self { server, id }
    }

    pub(crate) fn receive(&self, bytes: &[u8]) {
        let Some(message) = decode(bytes) else {
            return;
        };
        let delivery = self
            .server
            .voice
            .relay
            .lock()
            .handle(self.id, message, Instant::now());
        if let Some(delivery) = delivery {
            deliver(&self.server, delivery);
        }
    }
}

/// Audio is droppable: it never retires a connection and is not queued when the
/// listener's reliable queue is less than half free (headroom for control packets).
fn deliver(server: &EdgeServer, delivery: Delivery) {
    let Ok(framed) = frame(&delivery.payload) else {
        return;
    };
    for to in delivery.to {
        if let Some(handles) = server.connections.get(to)
            && handles.reliable_tx.capacity() * 2 >= handles.reliable_tx.max_capacity()
        {
            let _ = handles.reliable_tx.try_send(framed.clone());
        }
    }
}

pub(crate) fn spawn_tick_task(server: Arc<EdgeServer>) -> tokio::task::JoinHandle<()> {
    let mut ticks = server.hub.subscribe();
    tokio::spawn(async move {
        loop {
            match ticks.recv().await {
                Ok(tick) => server.voice.tick(&tick),
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
            }
        }
    })
}
