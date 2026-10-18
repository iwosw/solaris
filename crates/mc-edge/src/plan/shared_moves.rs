//! Shared per-snapshot entity move frames across viewers.
//!
//! One [`SharedMoves`] covers a single immutable snapshot publication:
//! every viewer plans from the same entity images, so the wire frame for
//! one `(entity id, exact viewer baseline)` pair is identical and encoded
//! once. Viewers keep their own baselines (credit deferral and late joins
//! diverge); only identical `(id, baseline)` pairs share a frame.
//!
//! Sharded `std::Mutex<FxHashMap>` keeps rayon viewers off a single lock.
//! Below-threshold (`None`) moves are cached too, skipping repeated
//! threshold math. A cache MUST be dropped with its snapshot: the entity
//! image is the implicit half of every key.

use super::*;
use rustc_hash::FxHashMap;
use std::sync::Mutex;

/// Shard count (power of two; the mask keeps shard selection cheap).
const SHARDS: usize = 64;

/// Exact `(entity id, viewer baseline)` key. Floats compare by bits: a
/// conservative exact match, so only truly identical baselines share.
/// All baseline fields are in the key even though the move math ignores
/// the head baseline; a stale-sharer would corrupt someone's delta.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct MoveKey {
    id: i32,
    px: u64,
    py: u64,
    pz: u64,
    yaw: u32,
    pitch: u32,
    head: u32,
}

impl MoveKey {
    fn new(id: EntityId, baseline: MoveBaseline) -> Self {
        let (pos, yaw, pitch, head) = baseline;
        Self {
            id: id.0,
            px: pos[0].to_bits(),
            py: pos[1].to_bits(),
            pz: pos[2].to_bits(),
            yaw: yaw.to_bits(),
            pitch: pitch.to_bits(),
            head: head.to_bits(),
        }
    }
}

/// One shared movement frame: packet id, framed wire bytes, and the
/// baseline the client holds once the frame is delivered.
#[derive(Debug, Clone)]
pub(super) struct SharedMove {
    pub(super) id: i32,
    pub(super) data: bytes::Bytes,
    pub(super) next: MoveBaseline,
}

/// Thread-safe move/head frame cache for one snapshot publication.
/// Share `&self` across rayon viewers; drop with the snapshot.
#[derive(Debug)]
pub(super) struct SharedMoves {
    moves: [Mutex<FxHashMap<MoveKey, Option<SharedMove>>>; SHARDS],
    heads: [Mutex<FxHashMap<i32, (i32, bytes::Bytes)>>; SHARDS],
}

impl Default for SharedMoves {
    fn default() -> Self {
        Self {
            moves: std::array::from_fn(|_| Mutex::new(FxHashMap::default())),
            heads: std::array::from_fn(|_| Mutex::new(FxHashMap::default())),
        }
    }
}

impl SharedMoves {
    /// Shared movement frame for `entity` against a viewer `baseline`;
    /// `None` when the move is below vanilla thresholds. A hit clones no
    /// body bytes; a miss encodes once through the shared `MovePacket`
    /// path and caches the frame (including `None`).
    pub(super) fn movement(
        &self,
        entity: &EntityView,
        baseline: MoveBaseline,
    ) -> Option<SharedMove> {
        let key = MoveKey::new(entity.id, baseline);
        let shard = &self.moves[(key.id as usize) & (SHARDS - 1)];
        let mut guard = shard.lock().expect("shared move encoder mutex");
        guard
            .entry(key)
            .or_insert_with(|| {
                entity_movement(entity, baseline).map(|(packet, next)| {
                    let (id, data) = match packet {
                        MovePacket::Pos(packet) => frame(&packet),
                        MovePacket::Rot(packet) => frame(&packet),
                        MovePacket::PosRot(packet) => frame(&packet),
                        MovePacket::Sync(packet) => frame(&packet),
                    };
                    SharedMove { id, data, next }
                })
            })
            .clone()
    }

    /// Shared `RotateHead` frame for `entity`'s current head yaw, keyed by
    /// entity id only: it never depends on a viewer's movement baseline.
    /// The caller wraps `(id, data)` into its own `PlannedPacket`.
    pub(super) fn head(&self, entity: &EntityView) -> (i32, bytes::Bytes) {
        let shard = &self.heads[(entity.id.0 as usize) & (SHARDS - 1)];
        let mut guard = shard.lock().expect("shared head encoder mutex");
        guard
            .entry(entity.id.0)
            .or_insert_with(|| {
                frame(&RotateHead {
                    entity_id: entity.id.0,
                    head_yaw: entity.head_yaw,
                })
            })
            .clone()
    }
}

/// Movement/head bodies are fixed-size numeric fields (at most 68 bytes).
/// Encode into stack storage, allocating only the final shared wire frame.
fn frame<P: Packet>(packet: &P) -> (i32, bytes::Bytes) {
    let mut body = [0_u8; 128];
    let mut remaining = &mut body[..];
    packet
        .encode(&mut remaining)
        .expect("numeric entity packet encodes");
    let len = 128 - remaining.len();
    let mut data = BytesMut::with_capacity(len + 11);
    append_frame(&mut data, P::ID, &body[..len], Compression::Threshold(256))
        .expect("numeric entity frame fits");
    (P::ID, data.freeze())
}
