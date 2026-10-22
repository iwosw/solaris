//! Independent Distant Horizons protocol-16 presentation for optional Java clients.
//! Terrain is sampled by the core on the shared LOD worker lane; no generator
//! keys, native databases, or authoritative world mutation belong to this module.
mod budget;
mod codec;
mod dto;
mod session;

use crate::config::EdgeConfig;
use crate::server::EdgeServer;
use budget::{Bucket, CACHE_BYTES, Cache, GLOBAL_JOBS};
use mc_core::{Core, dimensions::Dimension};
use mc_data::Identifier;
use mc_protocol::packets::{CustomPayload, Packet, play::ServerboundCustomPayload};
pub(crate) use session::{Connection, Outbound};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use tokio::sync::{Semaphore, watch};

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct Key {
    dimension: Dimension,
    position: i64,
    revision: u64,
}
struct Tile {
    bytes: Arc<Vec<u8>>,
    modified: i64,
}
type ResultTile = Result<Arc<Tile>, String>;
type Pending = watch::Receiver<Option<ResultTile>>;

pub(crate) struct Service {
    identity: String,
    workers: Arc<Semaphore>,
    slots: Arc<Semaphore>,
    cache: Mutex<Cache<Key, Tile>>,
    inflight: Mutex<HashMap<Key, Pending>>,
    bandwidth: Mutex<Bucket>,
    privacy: Option<crate::anti_xray::AntiXray>,
}
impl Service {
    pub(crate) fn new(
        hub: &Core,
        cfg: &EdgeConfig,
        workers: Arc<Semaphore>,
    ) -> anyhow::Result<Self> {
        let registry = hub.registry();
        Ok(Self {
            identity: hub.terrain_identity().to_owned(),
            workers,
            slots: Arc::new(Semaphore::new(GLOBAL_JOBS)),
            cache: Mutex::new(Cache::new(CACHE_BYTES)),
            inflight: Mutex::default(),
            bandwidth: Mutex::new(Bucket::new(2_000_000, 100_000)),
            privacy: crate::anti_xray::AntiXray::new(
                &registry.blocks,
                &registry.light,
                &cfg.anti_xray,
            )?,
        })
    }
    async fn tile(
        self: &Arc<Self>,
        server: Arc<EdgeServer>,
        dimension: Dimension,
        position: i64,
    ) -> ResultTile {
        let revision = server.hub.terrain_revision(native_dimension(dimension));
        if revision == u64::MAX {
            return Err("terrain publication unavailable; request again".into());
        }
        let key = Key {
            dimension,
            position,
            revision,
        };
        if let Some(tile) = self.cache.lock().expect("DH cache").get(&key) {
            return Ok(tile);
        }
        let mut receiver = {
            let mut inflight = self.inflight.lock().expect("DH in-flight");
            if let Some(tile) = self.cache.lock().expect("DH cache").get(&key) {
                return Ok(tile);
            }
            if let Some(receiver) = inflight.get(&key) {
                receiver.clone()
            } else {
                let permit = Arc::clone(&self.slots)
                    .try_acquire_owned()
                    .map_err(|_| "global DH section budget exhausted".to_owned())?;
                let (sender, receiver) = watch::channel(None);
                inflight.insert(key, receiver.clone());
                let service = Arc::clone(self);
                tokio::spawn(async move {
                    let _permit = permit;
                    let result = match Arc::clone(&service.workers).acquire_owned().await {
                        Ok(lane) => {
                            let worker = Arc::clone(&service);
                            tokio::task::spawn_blocking(move || {
                                let _lane = lane;
                                worker.convert(&server, key)
                            })
                            .await
                            .unwrap_or_else(|error| Err(format!("DH terrain worker: {error}")))
                        }
                        Err(_) => Err("LOD worker lane closed".into()),
                    };
                    if sender.receiver_count() > 1
                        && let Ok(tile) = &result
                    {
                        service.cache.lock().expect("DH cache").insert(
                            key,
                            Arc::clone(tile),
                            tile.bytes.len(),
                        );
                    }
                    sender.send_replace(Some(result));
                    service.inflight.lock().expect("DH in-flight").remove(&key);
                });
                receiver
            }
        };
        loop {
            if let Some(result) = receiver.borrow().clone() {
                return result;
            }
            receiver
                .changed()
                .await
                .map_err(|_| "DH terrain worker closed".to_owned())?;
        }
    }
    fn convert(&self, server: &EdgeServer, key: Key) -> ResultTile {
        let section = codec::Section::decode(key.position).map_err(|error| error.to_string())?;
        let terrain = server
            .hub
            .terrain_section(native_dimension(key.dimension), section.x, section.z)
            .map_err(|error| error.to_string())?;
        let mut chunks = terrain.chunks;
        if let Some(privacy) = &self.privacy {
            let mut snapshot: Vec<_> = chunks
                .iter()
                .map(|chunk| mc_core::boundary::SnapshotChunk {
                    pos: chunk.pos,
                    revision: 0,
                    chunk: Arc::clone(chunk),
                })
                .collect();
            snapshot.sort_unstable_by_key(|chunk| (chunk.pos.x, chunk.pos.z));
            chunks = snapshot
                .iter()
                .map(|chunk| {
                    let overrides = privacy.hide(
                        key.dimension,
                        &crate::anti_xray::Surroundings::of(&snapshot, chunk),
                    );
                    if overrides.is_empty() {
                        return Arc::clone(&chunk.chunk);
                    }
                    let mut copy = (*chunk.chunk).clone();
                    for (index, replacement) in overrides.into_iter().enumerate() {
                        if let Some(section) = replacement {
                            copy.sections[index] = section;
                        }
                    }
                    Arc::new(copy)
                })
                .collect();
        }
        let bytes = dto::encode(
            key.position,
            &chunks,
            &server.hub.registry().blocks,
            terrain.modified_ms,
        )
        .map_err(|error| error.to_string())?;
        if server.hub.terrain_revision(native_dimension(key.dimension)) != key.revision {
            return Err("terrain changed during DH sampling; request again".into());
        }
        Ok(Arc::new(Tile {
            bytes: Arc::new(bytes),
            modified: terrain.modified_ms,
        }))
    }
}

fn native_dimension(dimension: Dimension) -> u32 {
    match dimension {
        Dimension::Overworld => 0,
        Dimension::Nether => 1,
        Dimension::End => 2,
    }
}
fn channel() -> Identifier {
    Identifier::parse("distant_horizons:msg").expect("static DH channel")
}
pub(crate) fn advertisement() -> anyhow::Result<bytes::Bytes> {
    crate::lod::frame(
        Identifier::parse("minecraft:register")?,
        b"distant_horizons:msg".to_vec(),
    )
}
pub(crate) fn payload(body: &[u8]) -> Option<Vec<u8>> {
    let mut body = body;
    match ServerboundCustomPayload::decode(&mut body).ok()?.payload {
        CustomPayload::Unknown {
            channel: name,
            payload,
        } if body.is_empty() && name == channel() => Some(payload),
        _ => None,
    }
}
fn frame(message: &codec::Message) -> anyhow::Result<bytes::Bytes> {
    crate::lod::frame(channel(), codec::encode(message)?)
}
fn unix_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |time| {
            i64::try_from(time.as_millis()).unwrap_or(i64::MAX)
        })
}
