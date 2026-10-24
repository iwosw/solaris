use super::*;
use std::time::Duration;

struct Monitors(bool);
impl World for Monitors {
    fn is_monitor(&self, _: u8, _: [i32; 3], _: u8) -> bool {
        self.0
    }
}

const OWNER: ConnectionId = ConnectionId(1);
const VIEWER: ConnectionId = ConnectionId(2);
const BYSTANDER: ConnectionId = ConnectionId(3);

/// South face of a 4-wide row at x 10..14, z 10 (face plane z = 11).
fn row() -> ScreenId {
    ScreenId {
        pos: [10, 64, 10],
        side: 3,
        width: 4,
        height: 2,
        dimension: 0,
    }
}
fn at(x: f64, z: f64, yaw: f32) -> Pose {
    Pose::from_feet(0, [x, 63.0, z], yaw, 0.0)
}
/// Looking north (-z) from the south side, straight at the row.
fn watching() -> Pose {
    at(12.0, 18.0, 180.0)
}
fn away() -> Pose {
    at(12.0, 18.0, 0.0)
}
fn poses(viewer: Pose) -> HashMap<ConnectionId, Pose> {
    HashMap::from([
        (OWNER, at(12.0, 14.0, 180.0)),
        (VIEWER, viewer),
        (BYSTANDER, watching()),
    ])
}
fn relay() -> Relay {
    let mut relay = Relay::default();
    for (id, name) in [(OWNER, "owner"), (VIEWER, "viewer"), (BYSTANDER, "other")] {
        relay.register(
            id,
            EntityId(id.0 as i32),
            Uuid::from_u128(id.0.into()),
            name.into(),
        );
    }
    relay.tick(poses(watching()), &Monitors(true));
    relay
}
fn kinds(effects: &[Effect]) -> Vec<(u64, u8)> {
    effects.iter().map(|e| (e.to.0, e.payload[1])).collect()
}
fn open(relay: &mut Relay) -> Vec<Effect> {
    relay.handle(OWNER, Message::Open(row()), &Monitors(true), Instant::now())
}
/// Open the row and let VIEWER (looking `viewer`) be granted by OWNER.
fn granted(viewer: Pose) -> (Relay, Vec<Effect>) {
    let mut relay = relay();
    open(&mut relay);
    let now = Instant::now();
    let asked = relay.handle(VIEWER, Message::Request(row()), &Monitors(true), now);
    let request = u32::from_be_bytes(asked[0].payload[18..22].try_into().unwrap());
    relay.handle(
        OWNER,
        Message::Answer {
            request,
            allow: true,
        },
        &Monitors(true),
        now,
    );
    let effects = relay.tick(poses(viewer), &Monitors(true));
    (relay, effects)
}
fn part(index: u8, count: u8, data: &[u8]) -> Vec<u8> {
    let mut out = vec![1, 2];
    row().write(&mut out);
    out.extend(7u32.to_be_bytes());
    out.extend(320u16.to_be_bytes());
    out.extend(200u16.to_be_bytes());
    out.extend([0, index, count]);
    out.extend(data);
    out
}

#[test]
fn viewer_selection_uses_pose_side_range_and_direction() {
    let screen = row();
    assert!(looks_at(&watching(), &screen));
    // Slightly off-centre and from the side of the row still counts.
    assert!(looks_at(&at(20.0, 16.0, 160.0), &screen));
    assert!(!looks_at(&away(), &screen), "looking away");
    assert!(
        !looks_at(&at(12.0, 5.0, 0.0), &screen),
        "behind the face plane"
    );
    assert!(
        !looks_at(&at(12.0, 60.0, 180.0), &screen),
        "beyond view distance"
    );
    assert!(
        !looks_at(
            &Pose {
                dimension: 1,
                ..watching()
            },
            &screen
        ),
        "other dimension"
    );
}

#[test]
fn permission_then_looking_drives_demand_and_stop() {
    let mut relay = relay();
    let opened = open(&mut relay);
    assert_eq!(
        kinds(&opened),
        [(1, 0), (2, 6), (3, 6)],
        "owner DEMAND off, others ACTIVE"
    );
    let asked = relay.handle(
        VIEWER,
        Message::Request(row()),
        &Monitors(true),
        Instant::now(),
    );
    assert_eq!(kinds(&asked), [(1, 4)], "owner is asked");
    // Nobody is granted yet: looking players do not turn demand on.
    assert!(relay.tick(poses(watching()), &Monitors(true)).is_empty());
    let request = u32::from_be_bytes(asked[0].payload[18..22].try_into().unwrap());
    let answered = relay.handle(
        OWNER,
        Message::Answer {
            request,
            allow: true,
        },
        &Monitors(true),
        Instant::now(),
    );
    assert_eq!(kinds(&answered), [(2, 5)]);
    assert_eq!(answered[0].payload[18], 1);
    assert_eq!(
        kinds(&relay.tick(poses(watching()), &Monitors(true))),
        [(1, 0)]
    );
    assert!(
        relay.tick(poses(watching()), &Monitors(true)).is_empty(),
        "no repeat"
    );
    let aside = relay.tick(poses(away()), &Monitors(true));
    assert_eq!(
        kinds(&aside),
        [(2, 2), (1, 0)],
        "STOP viewer, DEMAND off owner"
    );
    assert_eq!(aside[0].payload[18], stop::LOOK_AWAY);
    assert_eq!(aside[1].payload[18], 0);
}

#[test]
fn frames_reach_only_looking_granted_viewers_reassembled() {
    let (mut relay, effects) = granted(watching());
    assert_eq!(kinds(&effects), [(1, 0)]);
    let now = Instant::now();
    let first = vec![0xAB; MAX_PART_BYTES];
    assert!(
        relay
            .handle(
                OWNER,
                decode(&part(0, 2, &first)).unwrap(),
                &Monitors(true),
                now
            )
            .is_empty()
    );
    let sent = relay.handle(
        OWNER,
        decode(&part(1, 2, b"tail")).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(
        kinds(&sent),
        [(2, 1)],
        "only the granted viewer, never the bystander"
    );
    let payload = &sent[0].payload;
    assert_eq!(&payload[18..22], 7u32.to_be_bytes());
    assert_eq!(&payload[22..26], [1, 0x40, 0, 200]);
    assert_eq!(payload[26], 0);
    assert_eq!(payload[27..].len(), MAX_PART_BYTES + 4);
    // Out-of-order continuation is dropped without a frame.
    assert!(
        relay
            .handle(
                OWNER,
                decode(&part(1, 2, b"x")).unwrap(),
                &Monitors(true),
                now
            )
            .is_empty()
    );
    // Three 90 KB frames at one instant: 262144 - 30004 leaves room for two.
    let mut delivered = 0;
    for _ in 0..3 {
        let mut sent = Vec::new();
        for index in 0..3 {
            sent = relay.handle(
                OWNER,
                decode(&part(index, 3, &first)).unwrap(),
                &Monitors(true),
                now,
            );
        }
        delivered += sent.len();
    }
    assert_eq!(delivered, 2, "the frame beyond the byte budget is dropped");
}

#[test]
fn oversize_or_unrequested_frames_close_the_screen() {
    let (mut relay, _) = granted(away());
    let now = Instant::now();
    // Demand is off: stray frames are dropped, then the screen is closed.
    let mut closed = Vec::new();
    for _ in 0..=MAX_STRIKES {
        closed = relay.handle(
            OWNER,
            decode(&part(0, 1, b"x")).unwrap(),
            &Monitors(true),
            now,
        );
    }
    assert_eq!(kinds(&closed)[0], (1, 3), "REFUSED bad frame to owner");
    let (mut relay, _) = granted(watching());
    let bad = relay.handle(
        OWNER,
        decode(&part(0, 9, b"x")).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(kinds(&bad)[0], (1, 3));
}

#[test]
fn revoke_leave_and_disconnects_end_permission() {
    // Owner revokes.
    let (mut relay, _) = granted(watching());
    let revoked = relay.handle(
        OWNER,
        Message::Revoke {
            id: row(),
            viewer: Uuid::from_u128(2),
        },
        &Monitors(true),
        Instant::now(),
    );
    assert_eq!(kinds(&revoked), [(2, 2), (1, 0)]);
    assert_eq!(revoked[0].payload[18], stop::REVOKED);
    assert!(
        relay.tick(poses(watching()), &Monitors(true)).is_empty(),
        "grant is gone"
    );

    // Viewer leaves.
    let (mut relay, _) = granted(watching());
    let left = relay.handle(
        VIEWER,
        Message::Leave(row()),
        &Monitors(true),
        Instant::now(),
    );
    assert_eq!(kinds(&left), [(1, 0)]);

    // Viewer disconnects while looking.
    let (mut relay, _) = granted(watching());
    assert_eq!(kinds(&relay.unregister(VIEWER)), [(1, 0)]);

    // Owner disconnects: viewer stopped, the dimension told it is idle.
    let (mut relay, _) = granted(watching());
    let gone = relay.unregister(OWNER);
    assert_eq!(kinds(&gone), [(2, 2), (2, 6), (3, 6)]);
    assert_eq!(gone[0].payload[18], stop::OWNER_LEFT);
    assert!(relay.tick(poses(watching()), &Monitors(true)).is_empty());

    // Owner closes the capture: grants end with the last screen.
    let (mut relay, _) = granted(watching());
    let closed = relay.handle(
        OWNER,
        Message::Close(row()),
        &Monitors(true),
        Instant::now(),
    );
    assert_eq!(closed[0].payload[18], stop::OWNER_CLOSED);
    open(&mut relay);
    assert!(
        relay.tick(poses(watching()), &Monitors(true)).is_empty(),
        "a new demonstration needs a new permission"
    );
}

#[test]
fn leave_cancels_an_unanswered_request() {
    let mut relay = relay();
    open(&mut relay);
    let now = Instant::now();
    let asked = relay.handle(VIEWER, Message::Request(row()), &Monitors(true), now);
    let request = u32::from_be_bytes(asked[0].payload[18..22].try_into().unwrap());
    assert!(
        relay
            .handle(VIEWER, Message::Leave(row()), &Monitors(true), now)
            .is_empty()
    );
    let answered = relay.handle(
        OWNER,
        Message::Answer {
            request,
            allow: true,
        },
        &Monitors(true),
        now,
    );
    assert!(answered.is_empty(), "a withdrawn request grants nothing");
    assert!(
        relay.tick(poses(watching()), &Monitors(true)).is_empty(),
        "no demand without a grant"
    );
}

#[test]
fn open_is_validated_and_exclusive() {
    let mut relay = relay();
    let now = Instant::now();
    let refusal = |effects: Vec<Effect>| (effects[0].payload[1], effects[0].payload[18]);
    assert_eq!(
        refusal(relay.handle(OWNER, Message::Open(row()), &Monitors(false), now)),
        (3, refused::NOT_MONITOR)
    );
    let far = ScreenId {
        pos: [60, 64, 10],
        ..row()
    };
    assert_eq!(
        refusal(relay.handle(OWNER, Message::Open(far), &Monitors(true), now)),
        (3, refused::OUT_OF_REACH)
    );
    assert!(!open(&mut relay).is_empty());
    relay.poses.insert(VIEWER, at(12.0, 12.0, 0.0));
    assert_eq!(
        refusal(relay.handle(VIEWER, Message::Open(row()), &Monitors(true), now)),
        (3, refused::TAKEN)
    );
    assert_eq!(
        kinds(&relay.handle(VIEWER, Message::Request(row()), &Monitors(true), now)),
        [(1, 4)]
    );
    assert_eq!(
        refusal(relay.handle(VIEWER, Message::Request(row()), &Monitors(true), now)),
        (3, refused::REPEAT)
    );
}

fn audio_part(seq: u32, opus: &[u8]) -> Vec<u8> {
    let mut out = vec![1, 7];
    row().write(&mut out);
    out.extend(seq.to_be_bytes());
    out.extend(opus);
    out
}

#[test]
fn audio_reaches_only_looking_granted_viewers_while_demanded() {
    let (mut relay, effects) = granted(watching());
    assert_eq!(kinds(&effects), [(1, 0)]);
    let now = Instant::now();
    let sent = relay.handle(
        OWNER,
        decode(&audio_part(9, &[0xF8, 1, 2, 3])).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(
        kinds(&sent),
        [(2, 7)],
        "granted viewer only, never the bystander"
    );
    assert_eq!(&sent[0].payload[18..22], 9u32.to_be_bytes());
    assert_eq!(&sent[0].payload[22..], [0xF8, 1, 2, 3]);
    assert!(sent[0].is_media());
    // The viewer looks away: demand off, so audio goes nowhere and strikes accrue.
    let aside = relay.tick(poses(away()), &Monitors(true));
    assert_eq!(kinds(&aside), [(2, 2), (1, 0)]);
    assert!(
        relay
            .handle(
                OWNER,
                decode(&audio_part(10, b"x")).unwrap(),
                &Monitors(true),
                now
            )
            .is_empty(),
        "no demand: dropped"
    );
    // Viewer comes back: audio flows again.
    relay.tick(poses(watching()), &Monitors(true));
    let sent = relay.handle(
        OWNER,
        decode(&audio_part(11, b"y")).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(kinds(&sent), [(2, 7)]);
}

#[test]
fn audio_from_non_owner_or_ungranted_is_never_relayed() {
    let (mut relay, _) = granted(watching());
    let now = Instant::now();
    let refused = relay.handle(
        VIEWER,
        decode(&audio_part(1, b"x")).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(
        (refused[0].payload[1], refused[0].payload[18]),
        (3, refused::NO_SCREEN)
    );
    // Permission ended: the grant is gone, so nobody watches and nothing is sent.
    relay.handle(VIEWER, Message::Leave(row()), &Monitors(true), now);
    assert!(
        relay
            .handle(
                OWNER,
                decode(&audio_part(2, b"x")).unwrap(),
                &Monitors(true),
                now
            )
            .iter()
            .all(|e| e.payload[1] != 7)
    );
}

#[test]
fn audio_is_bounded_rate_limited_and_strikes_close_the_screen() {
    let (mut relay, _) = granted(watching());
    let now = Instant::now();
    // Packet burst: 25 pass at one instant, the rest are dropped.
    let delivered: usize = (0..40)
        .map(|seq| {
            relay
                .handle(
                    OWNER,
                    decode(&audio_part(seq, &[7; 100])).unwrap(),
                    &Monitors(true),
                    now,
                )
                .len()
        })
        .sum();
    assert_eq!(delivered, 25);
    // Tokens refill with time.
    let later = now + Duration::from_millis(100);
    assert_eq!(
        relay
            .handle(
                OWNER,
                decode(&audio_part(50, &[7; 100])).unwrap(),
                &Monitors(true),
                later
            )
            .len(),
        1
    );
    // Oversize Opus packets close the screen.
    let big = relay.handle(
        OWNER,
        decode(&audio_part(51, &vec![7; MAX_OPUS_BYTES + 1])).unwrap(),
        &Monitors(true),
        later,
    );
    assert_eq!(kinds(&big)[0], (1, 3));
    // Without demand, unsolicited audio is dropped then the screen is closed.
    let (mut relay, _) = granted(away());
    let mut closed = Vec::new();
    for seq in 0..=MAX_AUDIO_STRIKES {
        closed = relay.handle(
            OWNER,
            decode(&audio_part(seq, b"x")).unwrap(),
            &Monitors(true),
            now,
        );
    }
    assert_eq!(kinds(&closed)[0], (1, 3));
    // Empty packets are malformed.
    let (mut relay, _) = granted(watching());
    let empty = relay.handle(
        OWNER,
        decode(&audio_part(1, b"")).unwrap(),
        &Monitors(true),
        now,
    );
    assert_eq!(kinds(&empty)[0], (1, 3));
}
