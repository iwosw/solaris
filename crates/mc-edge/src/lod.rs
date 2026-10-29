//! Far-terrain LOD for the native Solaris client (`[lod]`).
//!
//! Only a connection whose Loader session was negotiated as the native
//! platform gets `solaris:lod/hello` and may ask for tiles. Requests are
//! bounded per connection (`max_inflight_tiles`), by distance from the
//! player's last committed position (`max_distance`) and globally by
//! `workers` blocking threads. A tile that does not fit the connection's
//! world queue is dropped; the client asks again.
//!
//! Wire format (big-endian, protocol 1):
//! - hello: `u8 protocol, u8 flags, u32 max_distance, u16 max_inflight,
//!   i32 min_y, i32 height, i32 sea_level, u32 worldgen_revision`; flag bit
//!   0 = a program follows (`share_terrain_keys`)
//! - program: `u8 protocol`, then the zlib-compressed far-terrain program
//!   (`mc_worldgen::vanilla_density::program`); it reveals the seed's terrain
//!   noise, so it is sent only on operator opt-in. A client holding it asks
//!   for stored columns only and generates the rest itself.
//! - request: `u8 protocol, u8 count (1..=16), count * (i32 tile_x,
//!   i32 tile_z, u8 level, u8 flags)`; flag bit 0 = stored columns only
//! - tile: `u8 protocol, i32 tile_x, i32 tile_z, u8 level, u8 flags`, then
//!   256 samples (x fastest) of `u8 origin (0 missing, 1 stored, 2 router),
//!   i16 height, varint block state + 1 (0 none), varint biome id + 1 (0 none)`

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use mc_core::lod::{LodOrigin, TILE_EDGE};
use mc_data::Identifier;
use mc_protocol::packets::{
    CustomPayload, Packet,
    play::{ClientboundCustomPayload, ServerboundCustomPayload},
};

use crate::server::EdgeServer;

pub(crate) const PROTOCOL: u8 = 1;
const MAX_TILES_PER_REQUEST: usize = 16;

pub(crate) fn hello_channel() -> Identifier {
    Identifier::parse("solaris:lod/hello").expect("static lod channel")
}
pub(crate) fn request_channel() -> Identifier {
    Identifier::parse("solaris:lod/request").expect("static lod channel")
}
pub(crate) fn tile_channel() -> Identifier {
    Identifier::parse("solaris:lod/tile").expect("static lod channel")
}
pub(crate) fn program_channel() -> Identifier {
    Identifier::parse("solaris:lod/program").expect("static lod channel")
}

/// Vanilla's limit on a clientbound custom payload.
const MAX_PAYLOAD: usize = 1 << 20;

/// The Play-start frames for a native connection: the hello, then the
/// far-terrain program when the operator shares it and the generator has one.
pub(crate) fn hello_frames(server: &EdgeServer) -> anyhow::Result<Vec<bytes::Bytes>> {
    let cfg = &server.cfg.lod;
    let program = cfg
        .share_terrain_keys
        .then(|| server.hub.lod_program())
        .flatten()
        .filter(|program| {
            let fits = program.len() < MAX_PAYLOAD;
            if !fits {
                tracing::warn!(
                    bytes = program.len(),
                    "far-terrain program too large to send"
                );
            }
            fits
        });
    let mut frames = vec![frame(
        hello_channel(),
        encode_hello(cfg, program.is_some()),
    )?];
    if let Some(program) = program {
        let mut payload = Vec::with_capacity(program.len() + 1);
        payload.push(PROTOCOL);
        payload.extend_from_slice(&program);
        frames.push(frame(program_channel(), payload)?);
    }
    Ok(frames)
}

/// Server-wide LOD state: the tile worker budget and biome wire ids.
pub(crate) struct LodService {
    pub(crate) workers: Arc<tokio::sync::Semaphore>,
    biomes: HashMap<Identifier, u32>,
}

impl LodService {
    pub(crate) fn new(cfg: &crate::config::LodSection, biomes: &mc_data::Registry) -> Self {
        Self {
            workers: Arc::new(tokio::sync::Semaphore::new(cfg.workers.max(1))),
            biomes: biomes
                .entries
                .iter()
                .enumerate()
                .map(|(id, name)| (name.clone(), id as u32))
                .collect(),
        }
    }
}

/// One tile request as decoded from the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct TileRequest {
    pub tile_x: i32,
    pub tile_z: i32,
    pub level: u8,
    pub stored_only: bool,
}

pub(crate) fn decode_request(payload: &[u8]) -> anyhow::Result<Vec<TileRequest>> {
    let [protocol, count, tiles @ ..] = payload else {
        anyhow::bail!("short lod request");
    };
    anyhow::ensure!(*protocol == PROTOCOL, "unsupported lod protocol {protocol}");
    let count = usize::from(*count);
    anyhow::ensure!(
        (1..=MAX_TILES_PER_REQUEST).contains(&count) && tiles.len() == count * 10,
        "malformed lod request"
    );
    Ok(tiles
        .as_chunks::<10>()
        .0
        .iter()
        .map(|tile| TileRequest {
            tile_x: i32::from_be_bytes(tile[0..4].try_into().expect("4 bytes")),
            tile_z: i32::from_be_bytes(tile[4..8].try_into().expect("4 bytes")),
            level: tile[8],
            stored_only: tile[9] & 1 != 0,
        })
        .collect())
}

pub(crate) fn encode_hello(cfg: &crate::config::LodSection, program: bool) -> Vec<u8> {
    let geometry = mc_core::dimensions::Dimension::Overworld.geometry();
    let mut out = vec![PROTOCOL, u8::from(program)];
    out.extend(cfg.max_distance.to_be_bytes());
    out.extend(cfg.max_inflight_tiles.to_be_bytes());
    out.extend(geometry.min_y().to_be_bytes());
    out.extend(geometry.height().to_be_bytes());
    out.extend(63i32.to_be_bytes());
    out.extend(mc_worldgen::WORLDGEN_REVISION.to_be_bytes());
    out
}

pub(crate) fn encode_tile(
    request: TileRequest,
    samples: &[mc_core::lod::LodSample],
    biomes: &HashMap<Identifier, u32>,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(11 + samples.len() * 7);
    out.push(PROTOCOL);
    out.extend(request.tile_x.to_be_bytes());
    out.extend(request.tile_z.to_be_bytes());
    out.push(request.level);
    out.push(u8::from(request.stored_only));
    for sample in samples {
        out.push(match sample.origin {
            LodOrigin::Missing => 0,
            LodOrigin::Stored => 1,
            LodOrigin::Router => 2,
        });
        let height = i16::try_from(sample.height).unwrap_or(i16::MIN);
        out.extend(height.to_be_bytes());
        varint(&mut out, sample.block.map_or(0, |block| block.0 + 1));
        let biome = sample.biome.as_ref().and_then(|biome| biomes.get(biome));
        varint(&mut out, biome.map_or(0, |id| id + 1));
    }
    out
}

fn varint(out: &mut Vec<u8>, mut value: u32) {
    loop {
        if value < 0x80 {
            out.push(value as u8);
            return;
        }
        out.push((value as u8 & 0x7f) | 0x80);
        value >>= 7;
    }
}

/// The payload of a `solaris:lod/request` custom payload frame, else `None`.
pub(crate) fn request_payload(body: &[u8]) -> Option<Vec<u8>> {
    let mut body = body;
    match ServerboundCustomPayload::decode(&mut body).ok()?.payload {
        CustomPayload::Unknown { channel, payload }
            if body.is_empty() && channel == request_channel() =>
        {
            Some(payload)
        }
        _ => None,
    }
}

pub(crate) fn frame(channel: Identifier, payload: Vec<u8>) -> anyhow::Result<bytes::Bytes> {
    let mut body = Vec::new();
    ClientboundCustomPayload {
        payload: CustomPayload::Unknown { channel, payload },
    }
    .encode(&mut body)?;
    let mut frame = bytes::BytesMut::new();
    mc_protocol::frame::append_frame(
        &mut frame,
        ClientboundCustomPayload::ID,
        &body,
        mc_protocol::frame::Compression::Threshold(256),
    )?;
    Ok(frame.freeze())
}

/// Per-connection LOD state on the Play read loop.
pub(crate) struct LodConnection {
    inflight: Arc<AtomicUsize>,
    /// This connection's player in the root world's committed snapshots.
    entity: mc_core::boundary::EntityId,
    /// Committed tick outputs since the last request. Long-lived: a fresh
    /// subscriber sees only later outputs, never the current position.
    ticks: tokio::sync::broadcast::Receiver<mc_core::boundary::TickOutput>,
    /// Last committed native position (x, z); tiles are served around it.
    anchor: [f64; 2],
}

impl LodConnection {
    pub(crate) fn new(
        spawn: [f64; 3],
        entity: mc_core::boundary::EntityId,
        ticks: tokio::sync::broadcast::Receiver<mc_core::boundary::TickOutput>,
    ) -> Self {
        Self {
            inflight: Arc::default(),
            entity,
            ticks,
            anchor: [spawn[0], spawn[2]],
        }
    }

    /// Move the anchor to the newest committed position, never a rejected
    /// Move. Outputs without the player (another dimension) keep the anchor.
    fn follow(&mut self) {
        use tokio::sync::broadcast::error::TryRecvError;
        loop {
            match self.ticks.try_recv() {
                Ok(output) => {
                    if let Some(player) = output.snapshot.entities.get(self.entity) {
                        self.anchor = [player.position[0], player.position[2]];
                    }
                }
                Err(TryRecvError::Lagged(_)) => {}
                Err(TryRecvError::Empty | TryRecvError::Closed) => break,
            }
        }
    }

    /// Queue every admissible tile of one request; refused tiles are dropped.
    pub(crate) fn request(
        &mut self,
        server: &Arc<EdgeServer>,
        connection: mc_core::boundary::ConnectionId,
        payload: &[u8],
    ) -> anyhow::Result<()> {
        if server.hub.session_locked(connection) {
            return Ok(());
        }
        self.follow();
        let Some(service) = server.lod.as_ref() else {
            return Ok(());
        };
        let cfg = &server.cfg.lod;
        for request in decode_request(payload)? {
            if request.level > mc_core::lod::MAX_LEVEL || !self.near(request, cfg.max_distance) {
                continue;
            }
            if self.inflight.fetch_add(1, Ordering::AcqRel) >= usize::from(cfg.max_inflight_tiles) {
                self.inflight.fetch_sub(1, Ordering::AcqRel);
                continue;
            }
            let inflight = Arc::clone(&self.inflight);
            let workers = Arc::clone(&service.workers);
            let server = Arc::clone(server);
            tokio::spawn(async move {
                let _done = Done(inflight);
                let Ok(_permit) = workers.acquire_owned().await else {
                    return;
                };
                let hub = Arc::clone(&server.hub);
                let samples = tokio::task::spawn_blocking(move || {
                    hub.lod_tile(
                        request.tile_x,
                        request.tile_z,
                        request.level,
                        request.stored_only,
                    )
                })
                .await;
                let samples = match samples {
                    Ok(Ok(samples)) => samples,
                    Ok(Err(error)) => return tracing::warn!(%error, "lod tile failed"),
                    Err(error) => return tracing::warn!(%error, "lod tile task failed"),
                };
                if server.hub.session_locked(connection) {
                    return;
                }
                let Some(service) = server.lod.as_ref() else {
                    return;
                };
                let payload = encode_tile(request, &samples, &service.biomes);
                let Ok(frame) = frame(tile_channel(), payload) else {
                    return;
                };
                if let Some(handles) = server.connections.get(connection) {
                    let _ = handles.world_tx.try_send(frame);
                }
            });
        }
        Ok(())
    }

    /// Whether any part of the tile lies within `max_distance` of the anchor.
    fn near(&self, request: TileRequest, max_distance: u32) -> bool {
        let size = (TILE_EDGE as f64) * f64::from(1u32 << request.level);
        let gap = |tile: i32, anchor: f64| {
            let min = f64::from(tile) * size;
            (min - anchor).max(anchor - (min + size)).max(0.0)
        };
        let (dx, dz) = (
            gap(request.tile_x, self.anchor[0]),
            gap(request.tile_z, self.anchor[1]),
        );
        (dx * dx + dz * dz).sqrt() <= f64::from(max_distance)
    }
}

struct Done(Arc<AtomicUsize>);

impl Drop for Done {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_round_trips_and_rejects_malformed() {
        let mut payload = vec![PROTOCOL, 2];
        for (x, z, level, flags) in [(-3i32, 7i32, 4u8, 1u8), (100, -200, 1, 0)] {
            payload.extend(x.to_be_bytes());
            payload.extend(z.to_be_bytes());
            payload.extend([level, flags]);
        }
        let decoded = decode_request(&payload).unwrap();
        assert_eq!(
            decoded,
            [
                TileRequest {
                    tile_x: -3,
                    tile_z: 7,
                    level: 4,
                    stored_only: true
                },
                TileRequest {
                    tile_x: 100,
                    tile_z: -200,
                    level: 1,
                    stored_only: false
                }
            ]
        );
        assert!(decode_request(&payload[..payload.len() - 1]).is_err());
        assert!(decode_request(&[PROTOCOL, 0]).is_err());
        assert!(decode_request(&[2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]).is_err());
    }

    #[test]
    fn tile_encodes_every_sample_with_absent_fields_as_zero() {
        let biome = Identifier::parse("minecraft:plains").unwrap();
        let biomes = HashMap::from([(biome.clone(), 5)]);
        let samples = [
            mc_core::lod::LodSample {
                origin: LodOrigin::Stored,
                height: 70,
                block: Some(mc_world::BlockStateId(300)),
                biome: Some(biome),
            },
            mc_core::lod::LodSample {
                origin: LodOrigin::Missing,
                height: 0,
                block: None,
                biome: None,
            },
        ];
        let request = TileRequest {
            tile_x: 1,
            tile_z: -1,
            level: 2,
            stored_only: false,
        };
        let out = encode_tile(request, &samples, &biomes);
        assert_eq!(
            &out[..11],
            &[PROTOCOL, 0, 0, 0, 1, 255, 255, 255, 255, 2, 0]
        );
        // Stored: origin 1, height 70, block 301 as varint, biome 6.
        assert_eq!(&out[11..17], &[1, 0, 70, 0xad, 0x02, 6]);
        assert_eq!(&out[17..], &[0, 0, 0, 0, 0]);
    }

    #[test]
    fn distance_bound_measures_the_nearest_tile_point() {
        let (_, ticks) = tokio::sync::broadcast::channel(1);
        let lod = LodConnection::new([0.0, 64.0, 0.0], mc_core::boundary::EntityId(1), ticks);
        let tile = |tile_x, level| TileRequest {
            tile_x,
            tile_z: 0,
            level,
            stored_only: false,
        };
        // Level 4 tiles are 256 blocks wide: tile 8 starts at 2048.
        assert!(lod.near(tile(8, 4), 2048));
        assert!(!lod.near(tile(9, 4), 2048));
        assert!(lod.near(tile(-9, 4), 2048));
        assert!(!lod.near(tile(-10, 4), 2048));
    }

    /// Tiles are served around the player's newest committed position, read
    /// from outputs published after the connection opened (a subscriber made
    /// per request saw none and refused every tile).
    #[test]
    fn anchor_follows_committed_outputs_published_after_open() {
        use mc_core::boundary::{ChangeSet, EntityId, EntityView, TickOutput, WorldSnapshot};
        let output = |entities: Vec<EntityView>| TickOutput {
            tick: 1,
            snapshot: Arc::new(WorldSnapshot {
                entities: entities.into(),
                ..Default::default()
            }),
            changes: Arc::new(ChangeSet::default()),
            times: Default::default(),
        };
        let player = |x: f64| EntityView {
            id: EntityId(7),
            uuid: uuid::Uuid::nil(),
            type_id: 1,
            position: [x, 64.0, 0.0],
            velocity: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            head_yaw: 0.0,
            on_ground: true,
            appearance: Arc::default(),
        };
        let (tx, ticks) = tokio::sync::broadcast::channel(4);
        let mut lod = LodConnection::new([0.0, 64.0, 0.0], EntityId(7), ticks);
        let tile = |tile_x| TileRequest {
            tile_x,
            tile_z: 0,
            level: 4,
            stored_only: false,
        };
        tx.send(output(vec![player(10_000.0)])).unwrap();
        lod.follow();
        assert!(
            lod.near(tile(39), 2048),
            "served around the committed position"
        );
        assert!(!lod.near(tile(0), 2048), "no longer around spawn");
        // An output without the player (another dimension) keeps the anchor;
        // a lagged receiver skips to the newest output it still holds.
        tx.send(output(vec![])).unwrap();
        for x in [20_000.0, 30_000.0, 40_000.0, 50_000.0, 60_000.0] {
            tx.send(output(vec![player(x)])).unwrap();
        }
        lod.follow();
        assert!(lod.near(tile(234), 2048), "newest committed position wins");
    }

    #[test]
    fn hello_flags_the_program_only_when_shared() {
        let cfg = crate::config::LodSection::default();
        assert!(!cfg.share_terrain_keys, "key sharing is opt-in");
        let plain = encode_hello(&cfg, false);
        let shared = encode_hello(&cfg, true);
        assert_eq!(plain[..2], [PROTOCOL, 0]);
        assert_eq!(shared[..2], [PROTOCOL, 1]);
        assert_eq!(plain[2..], shared[2..]);
        // max_distance, max_inflight, min_y, height, sea_level, revision.
        assert_eq!(plain.len(), 2 + 4 + 2 + 4 + 4 + 4 + 4);
    }
}
