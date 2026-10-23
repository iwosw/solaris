use std::time::Duration;

use super::*;

const A: ConnectionId = ConnectionId(1);
const B: ConnectionId = ConnectionId(2);
const C: ConnectionId = ConnectionId(3);

fn pose(dimension: u8, x: f64) -> Pose {
    Pose::from_feet(dimension, [x, 64.0, 0.0], 0.0, 0.0)
}

/// A, B, C registered and listening at the given x offsets in dimension 0.
fn relay(xs: [f64; 3]) -> (Relay, Instant) {
    let now = Instant::now();
    let mut relay = Relay::new(&VoiceConfig::default());
    for (id, x) in [A, B, C].into_iter().zip(xs) {
        relay.register(id, EntityId(id.0 as i32), Uuid::from_u128(id.0.into()), now);
        relay.handle(id, Message::Listen(true), now);
        relay.poses.insert(id, pose(0, x));
    }
    (relay, now)
}
fn say(relay: &mut Relay, from: ConnectionId, now: Instant) -> Option<Delivery> {
    relay.handle(
        from,
        Message::Audio {
            seq: 7,
            opus: &[1, 2, 3],
        },
        now,
    )
}
fn sorted(delivery: Delivery) -> Vec<u64> {
    let mut to: Vec<u64> = delivery.to.iter().map(|id| id.0).collect();
    to.sort();
    to
}

#[test]
fn relays_only_within_range_and_never_to_the_speaker() {
    let (mut relay, now) = relay([0.0, 48.0, 48.5]);
    let delivery = say(&mut relay, A, now).unwrap();
    assert_eq!(sorted(delivery), vec![2]);
}

#[test]
fn other_dimension_is_not_relayed() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    relay.poses.insert(B, pose(1, 1.0));
    assert_eq!(sorted(say(&mut relay, A, now).unwrap()), vec![3]);
}

#[test]
fn only_listeners_with_a_pose_hear() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    relay.handle(B, Message::Listen(false), now);
    relay.poses.remove(&C);
    assert!(say(&mut relay, A, now).is_none());
    relay.handle(B, Message::Listen(true), now);
    assert_eq!(sorted(say(&mut relay, A, now).unwrap()), vec![2]);
}

#[test]
fn speaker_without_pose_or_not_registered_is_dropped() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    relay.poses.remove(&A);
    assert!(say(&mut relay, A, now).is_none());
    assert!(say(&mut relay, ConnectionId(9), now).is_none());
}

#[test]
fn audio_payload_carries_speaker_eye_and_seq_verbatim() {
    let (mut relay, now) = relay([10.0, 11.0, 200.0]);
    let payload = say(&mut relay, A, now).unwrap().payload;
    assert_eq!(&payload[..2], &[1, 0]);
    assert_eq!(&payload[2..18], Uuid::from_u128(1).as_bytes());
    assert_eq!(
        f32::from_be_bytes(payload[18..22].try_into().unwrap()),
        10.0
    );
    assert_eq!(
        f32::from_be_bytes(payload[22..26].try_into().unwrap()),
        65.62
    );
    assert_eq!(f32::from_be_bytes(payload[26..30].try_into().unwrap()), 0.0);
    assert_eq!(u32::from_be_bytes(payload[30..34].try_into().unwrap()), 7);
    assert_eq!(&payload[34..], &[1, 2, 3]);
}

#[test]
fn packet_rate_limit_drops_overload_then_refills() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    let sent = (0..40)
        .filter(|_| say(&mut relay, A, now).is_some())
        .count();
    assert_eq!(sent, 20);
    assert!(say(&mut relay, A, now + Duration::from_millis(100)).is_some());
}

#[test]
fn byte_rate_limit_drops_overload() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    let big = [0u8; MAX_OPUS_BYTES];
    let sent = (0..20)
        .filter(|_| {
            relay
                .handle(A, Message::Audio { seq: 0, opus: &big }, now)
                .is_some()
        })
        .count();
    assert_eq!(sent, 16);
    // A different speaker has its own budget.
    assert!(
        relay
            .handle(B, Message::Audio { seq: 0, opus: &big }, now)
            .is_some()
    );
}

#[test]
fn disabled_relays_nothing_and_reports_state() {
    let now = Instant::now();
    let mut relay = Relay::new(&VoiceConfig {
        enabled: false,
        range_blocks: 48,
    });
    let state = relay.register(A, EntityId(1), Uuid::nil(), now);
    assert_eq!(&state[..], &[1, 1, 0, 0, 48]);
    relay.register(B, EntityId(2), Uuid::nil(), now);
    relay.handle(B, Message::Listen(true), now);
    relay.poses.insert(A, pose(0, 0.0));
    relay.poses.insert(B, pose(0, 1.0));
    assert!(say(&mut relay, A, now).is_none());
}

#[test]
fn unregistered_connection_stops_hearing() {
    let (mut relay, now) = relay([0.0, 1.0, 2.0]);
    relay.unregister(B);
    assert_eq!(sorted(say(&mut relay, A, now).unwrap()), vec![3]);
}

#[test]
fn decode_validates_shape() {
    assert_eq!(
        decode(&[1, 0, 0, 0, 0, 9, 5, 6]),
        Some(Message::Audio {
            seq: 9,
            opus: &[5, 6]
        })
    );
    assert_eq!(decode(&[1, 1, 1]), Some(Message::Listen(true)));
    assert_eq!(decode(&[1, 1, 0]), Some(Message::Listen(false)));
    assert_eq!(decode(&[1, 1, 2]), None);
    assert_eq!(decode(&[2, 0, 0, 0, 0, 9, 5]), None, "version");
    assert_eq!(decode(&[1, 0, 0, 0, 0, 9]), None, "empty opus");
    let mut long = vec![1, 0, 0, 0, 0, 9];
    long.extend([0; MAX_OPUS_BYTES + 1]);
    assert_eq!(decode(&long), None, "oversized opus");
    assert_eq!(decode(&[1, 9]), None, "unknown kind");
    assert_eq!(decode(&[1]), None);
}

#[test]
fn default_config_parses_and_validates() {
    let config: VoiceConfig = toml::from_str("").unwrap();
    assert!(config.enabled);
    assert_eq!(config.range_blocks, 48);
    assert!(
        VoiceConfig {
            enabled: true,
            range_blocks: 0
        }
        .validate()
        .is_err()
    );
}
