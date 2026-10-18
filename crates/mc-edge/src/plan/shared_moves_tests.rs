//! Shared move-frame oracle: [`SharedMoves`] matches the per-viewer
//! `entity_movement` + `MovePacket::push` bytes exactly and shares frames
//! across identical `(entity id, baseline)` pairs.
use super::*;
use crate::plan::shared_moves::SharedMoves;

type Baseline = ([f64; 3], f32, f32, f32);

fn baseline_at(pos: [f64; 3], yaw: f32, pitch: f32, head: f32) -> Baseline {
    (pos, yaw, pitch, head)
}

/// Old per-viewer path: `entity_movement` framed for one connection.
fn oracle(entity: &EntityView, baseline: Baseline) -> Option<(i32, bytes::Bytes, Baseline)> {
    let (packet, next) = entity_movement(entity, baseline)?;
    let mut out = Vec::new();
    packet.push(&mut out, ConnectionId(7));
    assert_eq!(out.len(), 1);
    let framed = out.pop().expect("move frame fits the framing layer");
    Some((framed.id, framed.data, next))
}

fn frame_body(data: &bytes::Bytes) -> bytes::Bytes {
    let mut framed = bytes::BytesMut::from(&data[..]);
    mc_protocol::frame::try_decode_frame(
        &mut framed,
        mc_protocol::frame::Compression::Threshold(256),
    )
    .expect("shared frame decodes")
    .expect("shared frame is complete")
    .body
}

#[test]
fn shared_moves_is_send_sync_for_rayon_viewers() {
    fn bound<T: Send + Sync>() {}
    bound::<SharedMoves>();
}

#[test]
fn relative_pos_move_matches_oracle_and_shares_frame_bytes() {
    let shared = SharedMoves::default();
    let entity = entity(2, [8.5, 64.0, 8.0]);
    let baseline = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    let first = shared.movement(&entity, baseline).expect("moved");
    let second = shared.movement(&entity, baseline).expect("moved");
    let (id, data, next) = oracle(&entity, baseline).expect("moved");
    assert_eq!(first.id, id);
    assert_eq!(first.data, data);
    assert_eq!(first.next, next);
    // Second hit shares the frame without copying body bytes.
    assert_eq!(first.data.as_ptr(), second.data.as_ptr());
    assert_eq!(second.data, data);
    assert_eq!(second.next, next);
}

#[test]
fn rotation_matches_oracle() {
    let shared = SharedMoves::default();
    let mut entity = entity(3, [8.0, 64.0, 8.0]);
    entity.yaw = 90.0;
    let baseline = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    let got = shared.movement(&entity, baseline).expect("rotated");
    let (id, data, next) = oracle(&entity, baseline).expect("rotated");
    assert_eq!(id, MoveEntityRot::ID);
    assert_eq!(got.id, id);
    assert_eq!(got.data, data);
    assert_eq!(got.next, next);
}

#[test]
fn absolute_sync_matches_oracle() {
    let shared = SharedMoves::default();
    let entity = entity(4, [108.0, 64.0, 8.0]);
    let baseline = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    let got = shared.movement(&entity, baseline).expect("teleported");
    let (id, data, next) = oracle(&entity, baseline).expect("teleported");
    assert_eq!(id, EntityPositionSync::ID);
    assert_eq!(got.id, id);
    assert_eq!(got.data, data);
    assert_eq!(got.next, next);
}

#[test]
fn distinct_baselines_give_valid_distinct_deltas() {
    let shared = SharedMoves::default();
    let mut entity = entity(5, [8.5, 64.0, 8.0]);
    entity.yaw = 10.0;
    // Baselines differ in position (distinct deltas) and in yaw bits that
    // still pack to one step (10.0/10.5 both pack to 7: no rotation, but
    // the unrotated `next` yaw keeps each baseline's own value).
    let near = baseline_at([8.0, 64.0, 8.0], 10.0, 0.0, 0.0);
    let mid = baseline_at([8.25, 64.0, 8.0], 10.5, 0.0, 0.0);
    let from_near = shared.movement(&entity, near).expect("moved");
    let from_mid = shared.movement(&entity, mid).expect("moved");
    // Both match the old path for their own baseline.
    let (_, near_data, near_next) = oracle(&entity, near).expect("moved");
    let (_, mid_data, mid_next) = oracle(&entity, mid).expect("moved");
    assert_eq!(
        (from_near.data.clone(), from_near.next),
        (near_data, near_next)
    );
    assert_eq!((from_mid.data.clone(), from_mid.next), (mid_data, mid_next));
    // Distinct baselines stay distinct: 0.5 -> 2048, 0.25 -> 1024, and the
    // kept yaw values differ.
    assert_ne!(from_near.data, from_mid.data);
    assert_ne!(from_near.next, from_mid.next);
    let delta_near =
        MoveEntityPos::decode(&mut &frame_body(&from_near.data)[..]).expect("pos body");
    let delta_mid = MoveEntityPos::decode(&mut &frame_body(&from_mid.data)[..]).expect("pos body");
    assert_eq!((delta_near.delta_x, delta_mid.delta_x), (2048, 1024));
}

#[test]
fn head_baseline_field_never_conflates_movement_frames() {
    let shared = SharedMoves::default();
    let mut entity = entity(6, [8.5, 64.0, 8.0]);
    entity.head_yaw = 45.0;
    // Same move baselines, different viewer head baselines: the move math
    // ignores head, so both frames stay valid and equal.
    let low = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    let high = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 180.0);
    let from_low = shared.movement(&entity, low).expect("moved");
    let from_high = shared.movement(&entity, high).expect("moved");
    let (_, data, next) = oracle(&entity, low).expect("moved");
    assert_eq!(from_low.data, data);
    assert_eq!(from_high.data, data);
    assert_eq!(from_low.next, next);
    assert_eq!(from_high.next, next);
}

#[test]
fn no_move_returns_none_and_caches_it() {
    let shared = SharedMoves::default();
    let entity = entity(7, [8.0, 64.0, 8.0]);
    let baseline = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    assert!(oracle(&entity, baseline).is_none());
    assert!(shared.movement(&entity, baseline).is_none());
    // Second call hits the cached `None`, skipping threshold math again.
    assert!(shared.movement(&entity, baseline).is_none());
}

#[test]
fn head_frame_matches_oracle_and_is_shared() {
    let shared = SharedMoves::default();
    let mut entity = entity(8, [8.0, 64.0, 8.0]);
    entity.head_yaw = 45.0;
    let (id, data) = shared.head(&entity);
    let (again_id, again_data) = shared.head(&entity);
    let mut out = Vec::new();
    push_best_effort(
        &mut out,
        ConnectionId(9),
        &RotateHead {
            entity_id: entity.id.0,
            head_yaw: entity.head_yaw,
        },
    );
    assert_eq!(out.len(), 1);
    assert_eq!(id, RotateHead::ID);
    assert_eq!(id, out[0].id);
    assert_eq!(data, out[0].data);
    assert_eq!(again_id, id);
    assert_eq!(again_data.as_ptr(), data.as_ptr());
}

#[test]
fn concurrent_first_users_share_one_encoded_frame() {
    let shared = SharedMoves::default();
    let entity = entity(2, [8.5, 64.0, 8.0]);
    let baseline = baseline_at([8.0, 64.0, 8.0], 0.0, 0.0, 0.0);
    let barrier = std::sync::Barrier::new(8);
    let frames = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| {
                scope.spawn(|| {
                    barrier.wait();
                    shared.movement(&entity, baseline).unwrap().data
                })
            })
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap())
            .collect::<Vec<_>>()
    });
    assert!(
        frames
            .iter()
            .all(|frame| frame.as_ptr() == frames[0].as_ptr())
    );
}
