//! Regression tests for the protocol-16 codec. All fixtures are
//! independently hand-computed byte expectations (big-endian, u16 byte-count
//! DH strings), not encoder roundtrips.

use super::*;

fn head(ty: u16) -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(&PROTOCOL.to_be_bytes());
    v.extend_from_slice(&ty.to_be_bytes());
    v
}

fn with_body(ty: u16, body: &[u8]) -> Vec<u8> {
    let mut v = head(ty);
    v.extend_from_slice(body);
    v
}

fn dh_str(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(2 + s.len());
    v.extend_from_slice(&(s.len() as u16).to_be_bytes());
    v.extend_from_slice(s.as_bytes());
    v
}

#[test]
fn close_fixture_and_roundtrip() {
    let mut body = dh_str("bye");
    let wire = with_body(1, &body);
    assert_eq!(wire, b"\x00\x10\x00\x01\x00\x03bye");
    assert_eq!(decode(&wire).unwrap(), Message::Close("bye".into()));
    body.clear();
    body.extend(dh_str(""));
    assert_eq!(
        decode(&with_body(1, &body)).unwrap(),
        Message::Close("".into())
    );
    assert_eq!(encode(&Message::Close("bye".into())).unwrap(), wire);
}

#[test]
fn level_init_fixture() {
    let mut body = dh_str("minecraft:overworld");
    body.extend(dh_str("srv"));
    body.extend(dh_str("lvl"));
    body.extend_from_slice(&1234i64.to_be_bytes());
    let wire = with_body(2, &body);
    let msg = decode(&wire).unwrap();
    assert_eq!(
        msg,
        Message::LevelInit {
            dimension: "minecraft:overworld".into(),
            server_key: "srv".into(),
            level_key: "lvl".into(),
            time: 1234,
        }
    );
    assert_eq!(encode(&msg).unwrap(), wire);
}

#[test]
fn request_level_init_fixture() {
    let body = dh_str("minecraft:the_nether");
    let wire = with_body(3, &body);
    assert_eq!(
        decode(&wire).unwrap(),
        Message::RequestLevelInit("minecraft:the_nether".into())
    );
    assert_eq!(
        encode(&Message::RequestLevelInit("minecraft:the_nether".into())).unwrap(),
        wire
    );
}

#[test]
fn cancel_and_exception_fixtures() {
    let mut cancel = Vec::new();
    cancel.extend_from_slice(&0x8000_0001u32.to_be_bytes());
    let wire = with_body(5, &cancel);
    // The 32-bit trackable id keeps bit 31: no replacement with a 64-bit field.
    assert_eq!(decode(&wire).unwrap(), Message::Cancel(i32::MIN + 1));
    assert_eq!(encode(&Message::Cancel(i32::MIN + 1)).unwrap(), wire);

    let mut body = Vec::new();
    body.extend_from_slice(&7i32.to_be_bytes());
    body.extend_from_slice(&ERR_NEEDS_SPLIT.to_be_bytes());
    body.extend(dh_str("too coarse"));
    let wire = with_body(6, &body);
    let msg = Message::Exception {
        id: 7,
        kind: 3,
        message: "too coarse".into(),
    };
    assert_eq!(decode(&wire).unwrap(), msg);
    assert_eq!(encode(&msg).unwrap(), wire);
}

fn test_config() -> Config {
    Config {
        plan: PLAN_CHUNKS_ONLY,
        distance: 128,
        center_x: 10,
        center_z: -20,
        radius: 128,
        requests: 2,
        updates: true,
        update_radius: 8,
        sync: true,
        sync_distance: 64,
        sync_requests: 2,
        bandwidth: 500,
    }
}

#[test]
fn config_body_is_39_bytes_and_ordered() {
    let wire = encode(&Message::Config(test_config())).unwrap();
    assert_eq!(wire.len(), 4 + 39);
    let body = &wire[4..];
    let mut expect = vec![PLAN_CHUNKS_ONLY];
    expect.extend_from_slice(&128i32.to_be_bytes());
    expect.extend_from_slice(&10i32.to_be_bytes());
    expect.extend_from_slice(&(-20i32).to_be_bytes());
    expect.extend_from_slice(&128i32.to_be_bytes());
    expect.extend_from_slice(&2i32.to_be_bytes());
    expect.push(1);
    expect.extend_from_slice(&8i32.to_be_bytes());
    expect.push(1);
    expect.extend_from_slice(&64i32.to_be_bytes());
    expect.extend_from_slice(&2i32.to_be_bytes());
    expect.extend_from_slice(&500i32.to_be_bytes());
    assert_eq!(body, expect.as_slice());
    assert_eq!(decode(&wire).unwrap(), Message::Config(test_config()));
}

#[test]
fn config_rejects_bad_plan_and_negatives() {
    let mut bad = test_config();
    bad.plan = 9;
    assert!(encode(&Message::Config(bad.clone())).is_err());
    assert!(bad.validate().is_err());
    for (mut cfg, field) in [
        (test_config(), "distance"),
        (test_config(), "radius"),
        (test_config(), "requests"),
        (test_config(), "update_radius"),
        (test_config(), "sync_distance"),
        (test_config(), "sync_requests"),
        (test_config(), "bandwidth"),
    ] {
        match field {
            "distance" => cfg.distance = -1,
            "radius" => cfg.radius = -1,
            "requests" => cfg.requests = -1,
            "update_radius" => cfg.update_radius = -1,
            "sync_distance" => cfg.sync_distance = -1,
            "sync_requests" => cfg.sync_requests = -1,
            _ => cfg.bandwidth = -1,
        }
        assert!(cfg.validate().is_err(), "{field} must reject negatives");
        assert!(test_config().try_constrain(&cfg).is_err());
        assert!(cfg.try_constrain(&test_config()).is_err());
    }
    // Zero radius/bandwidth are unlimited, not invalid.
    let mut unlimited = test_config();
    unlimited.radius = 0;
    unlimited.bandwidth = 0;
    unlimited.validate().unwrap();
}

#[test]
fn config_constrain_intersection() {
    let server = test_config();
    let mut client = test_config();
    client.plan = PLAN_SURFACE_ONLY;
    client.distance = 512;
    client.requests = 20;
    client.updates = false;
    client.sync_distance = 9;
    client.bandwidth = 0;
    // Conflicting restricted plans use the server's plan, as Java does.
    let out = server.try_constrain(&client).unwrap();
    assert_eq!(out.plan, PLAN_CHUNKS_ONLY);
    assert_eq!(out.distance, 128);
    assert_eq!(out.requests, 2);
    assert!(!out.updates);
    assert!(out.sync);
    assert_eq!(out.sync_distance, 9);
    assert_eq!(out.bandwidth, 500);
    // Server-owned center/radius survive.
    assert_eq!((out.center_x, out.center_z, out.radius), (10, -20, 128));

    // SURFACE_THEN_CHUNKS is the permissive side; both-zero bandwidth stays 0.
    let mut permissive = test_config();
    permissive.plan = PLAN_SURFACE_THEN_CHUNKS;
    let mut other = test_config();
    other.plan = PLAN_SURFACE_ONLY;
    other.bandwidth = 0;
    permissive.bandwidth = 0;
    let out = permissive.try_constrain(&other).unwrap();
    assert_eq!(out.plan, PLAN_SURFACE_ONLY);
    assert_eq!(out.bandwidth, 0);
    let mut a = test_config();
    let mut b = test_config();
    a.bandwidth = 1000;
    b.bandwidth = 250;
    assert_eq!(a.try_constrain(&b).unwrap().bandwidth, 250);
}

#[test]
fn request_with_and_without_timestamp() {
    let section = Section {
        detail: 6,
        x: -3,
        z: 5,
    };
    let packed = section.packed();
    assert_eq!(Section::decode(packed).unwrap(), section);

    let bare = Message::Request {
        id: 42,
        level: "lvl".into(),
        position: packed,
        timestamp: None,
    };
    let wire = encode(&bare).unwrap();
    assert_eq!(wire[wire.len() - 1], 0, "absent timestamp is a zero flag");
    assert_eq!(decode(&wire).unwrap(), bare);

    let mut body = Vec::new();
    body.extend_from_slice(&42i32.to_be_bytes());
    body.extend(dh_str("lvl"));
    body.extend_from_slice(&packed.to_be_bytes());
    body.push(1);
    body.extend_from_slice(&987i64.to_be_bytes());
    let wire = with_body(7, &body);
    assert_eq!(
        decode(&wire).unwrap(),
        Message::Request {
            id: 42,
            level: "lvl".into(),
            position: packed,
            timestamp: Some(987),
        }
    );
    // A set present-flag with no following value is truncation, not None.
    let truncated = &wire[..wire.len() - 8];
    assert!(decode(truncated).is_err());
}

#[test]
fn response_none_and_some_with_beacons() {
    let mut body = Vec::new();
    body.extend_from_slice(&11i32.to_be_bytes());
    body.push(0);
    let wire = with_body(8, &body);
    assert_eq!(
        decode(&wire).unwrap(),
        Message::Response {
            id: 11,
            buffer: None
        }
    );
    assert_eq!(
        encode(&Message::Response {
            id: 11,
            buffer: None
        })
        .unwrap(),
        wire
    );

    // Payload with one bounded beacon entry (3x i32 coords + i32 color).
    let mut body = Vec::new();
    body.extend_from_slice(&11i32.to_be_bytes());
    body.push(1);
    body.extend_from_slice(&77i32.to_be_bytes());
    body.extend_from_slice(&1i32.to_be_bytes());
    body.extend_from_slice(&2i32.to_be_bytes());
    body.extend_from_slice(&3i32.to_be_bytes());
    body.extend_from_slice(&4i32.to_be_bytes());
    body.extend_from_slice(&9i32.to_be_bytes());
    let wire = with_body(8, &body);
    assert_eq!(
        decode(&wire).unwrap(),
        Message::Response {
            id: 11,
            buffer: Some(77),
        }
    );
    // Encode writes an empty beacon collection, never the skipped bytes.
    let out = encode(&Message::Response {
        id: 11,
        buffer: Some(77),
    })
    .unwrap();
    assert!(out.len() < wire.len());
    assert_eq!(out[out.len() - 4..], [0, 0, 0, 0]);
    assert_eq!(
        decode(&out).unwrap(),
        Message::Response {
            id: 11,
            buffer: Some(77),
        }
    );

    // Negative beacon count rejected before allocation.
    let mut body = Vec::new();
    body.extend_from_slice(&11i32.to_be_bytes());
    body.push(1);
    body.extend_from_slice(&77i32.to_be_bytes());
    body.extend_from_slice(&(-1i32).to_be_bytes());
    assert!(decode(&with_body(8, &body)).is_err());
}

#[test]
fn update_fixture() {
    let mut body = dh_str("lvl");
    body.extend_from_slice(&5i32.to_be_bytes());
    body.extend_from_slice(&0i32.to_be_bytes());
    let wire = with_body(9, &body);
    let msg = Message::Update {
        level: "lvl".into(),
        buffer: 5,
    };
    assert_eq!(decode(&wire).unwrap(), msg);
    assert_eq!(encode(&msg).unwrap(), wire);
}

#[test]
fn split_first_and_last_frames() {
    let first = Message::Split {
        buffer: 5,
        data: vec![1, 2, 3],
        first: true,
    };
    let mut body = Vec::new();
    body.extend_from_slice(&5i32.to_be_bytes());
    body.extend_from_slice(&3i32.to_be_bytes());
    body.extend_from_slice(&[1, 2, 3]);
    body.push(1);
    assert_eq!(encode(&first).unwrap(), with_body(10, &body));
    assert_eq!(decode(&with_body(10, &body)).unwrap(), first);

    let last = Message::Split {
        buffer: 5,
        data: vec![4, 5],
        first: false,
    };
    let wire = encode(&last).unwrap();
    assert_eq!(wire[wire.len() - 1], 0);
    assert_eq!(decode(&wire).unwrap(), last);
}

#[test]
fn section_packing_and_leaf_bounds() {
    let leaf = Section {
        detail: 6,
        x: 0,
        z: 0,
    };
    assert_eq!(leaf.packed(), 6);
    assert_eq!(Section::decode(6).unwrap(), leaf);
    // Inclusive max: the x0/z0 leaf spans 0..=63 on both axes.
    assert_eq!(leaf.bounds().unwrap(), ([0, 0], [63, 63]));

    let neg = Section {
        detail: 6,
        x: -1,
        z: -2,
    };
    assert_eq!(Section::decode(neg.packed()).unwrap(), neg);
    assert_eq!(neg.bounds().unwrap(), ([-64, -128], [-1, -65]));

    let coarse = Section {
        detail: 9,
        x: 1,
        z: -1,
    };
    // Detail > 6 decodes for the session's split path.
    assert_eq!(Section::decode(coarse.packed()).unwrap(), coarse);
    assert_eq!(coarse.bounds().unwrap(), ([512, -512], [1023, -1]));

    // World border and overflow are rejected.
    assert!(
        Section {
            detail: 6,
            x: 500_000,
            z: 0
        }
        .bounds()
        .is_err()
    );
    assert!(
        Section {
            detail: 31,
            x: 1,
            z: 0
        }
        .bounds()
        .is_err()
    );
}

#[test]
fn utf8_strings_use_byte_counts() {
    let s = "é";
    assert_eq!(s.len(), 2, "fixture needs a 2-byte char");
    let msg = Message::Close(s.into());
    let wire = encode(&msg).unwrap();
    assert_eq!(&wire[4..6], &[0, 2]);
    assert_eq!(decode(&wire).unwrap(), msg);
    // 2 chars / 4 bytes: a char count would read the wrong length.
    let multi = Message::RequestLevelInit("éé".into());
    let wire = encode(&multi).unwrap();
    assert_eq!(&wire[4..6], &[0, 4]);
    assert_eq!(decode(&wire).unwrap(), multi);
}

#[test]
fn rejects_protocol_type_length_trailing() {
    // Wrong protocol: dedicated understandable error naming protocol 16.
    let mut bad = head(1);
    bad.extend(dh_str("x"));
    bad[0] = 0;
    bad[1] = 15;
    let err = format!("{}", decode(&bad).unwrap_err());
    assert!(err.contains("protocol 16"), "unexpected error: {err}");

    // Unknown type, truncated body, trailing byte, bad split length/bool.
    assert!(decode(&head(99)).is_err());
    assert!(decode(&head(5)).is_err());
    let mut wire = encode(&Message::Cancel(3)).unwrap();
    wire.push(0);
    let err = format!("{}", decode(&wire).unwrap_err());
    assert!(err.contains("trailing"), "unexpected error: {err}");

    // Negative split length, then a valid length with an invalid bool.
    let mut body = Vec::new();
    body.extend_from_slice(&1i32.to_be_bytes());
    body.extend_from_slice(&(-2i32).to_be_bytes());
    body.push(1);
    assert!(decode(&with_body(10, &body)).is_err());
    let mut body = Vec::new();
    body.extend_from_slice(&1i32.to_be_bytes());
    body.extend_from_slice(&0i32.to_be_bytes());
    body.push(7);
    assert!(decode(&with_body(10, &body)).is_err());

    // Empty input and short envelope.
    assert!(decode(&[]).is_err());
    assert!(decode(&[0, 16]).is_err());
}

#[test]
fn impossible_section_details_fail_without_integer_overflow() {
    for detail in [25, 62, 63, 64, 127, 255] {
        assert!(
            Section { detail, x: 0, z: 0 }.bounds().is_err(),
            "detail {detail}"
        );
    }
}

#[test]
fn pinned_java_serializer_fixtures_work_in_both_directions() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("protocol16-java-fixtures.json")).unwrap();
    let position = Section {
        detail: LEAF_DETAIL,
        x: -1,
        z: -2,
    }
    .packed();
    let cases = [
        ("close", Message::Close("bye".into())),
        (
            "level_init",
            Message::LevelInit {
                dimension: "minecraft:overworld".into(),
                server_key: "srv".into(),
                level_key: "lvl".into(),
                time: 1234,
            },
        ),
        (
            "request_init",
            Message::RequestLevelInit("minecraft:the_nether".into()),
        ),
        ("config", Message::Config(test_config())),
        ("cancel_high_bit", Message::Cancel(i32::MIN + 1)),
        (
            "exception",
            Message::Exception {
                id: 7,
                kind: ERR_NEEDS_SPLIT,
                message: "too coarse".into(),
            },
        ),
        (
            "request_timestamp",
            Message::Request {
                id: i32::MIN + 1,
                level: "lvl".into(),
                position,
                timestamp: Some(1234),
            },
        ),
        (
            "request_no_timestamp",
            Message::Request {
                id: i32::MIN + 1,
                level: "lvl".into(),
                position,
                timestamp: None,
            },
        ),
        (
            "response",
            Message::Response {
                id: i32::MIN + 1,
                buffer: Some(7),
            },
        ),
        (
            "update",
            Message::Update {
                level: "lvl".into(),
                buffer: 7,
            },
        ),
        (
            "split",
            Message::Split {
                buffer: 7,
                data: b"ABC".to_vec(),
                first: true,
            },
        ),
    ];
    for (name, expected) in cases {
        let hex = oracle["frames"][name].as_str().unwrap();
        let wire: Vec<u8> = hex
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap())
            .collect();
        assert_eq!(decode(&wire).unwrap(), expected, "Java-to-Rust {name}");
        assert_eq!(encode(&expected).unwrap(), wire, "Rust-to-Java {name}");
    }
}

#[test]
fn conflicting_generation_plans_use_the_pinned_java_server_override() {
    let oracle: serde_json::Value =
        serde_json::from_str(include_str!("protocol16-java-fixtures.json")).unwrap();
    let server = test_config();
    let mut client = test_config();
    client.plan = PLAN_SURFACE_ONLY;
    assert_eq!(
        u64::from(server.constrain(&client).plan),
        oracle["server_chunk_client_surface_plan"].as_u64().unwrap()
    );
}
