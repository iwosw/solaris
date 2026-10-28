//! Session phases and frame-level translation over synthetic packet tables.

use bytes::{Bytes, BytesMut};
use mc_protocol::{Compression, RawFrame};

use super::tables::{Packets, Reports};
use super::*;

const CANONICAL: &str = r#"{
    "login": {
        "clientbound": {"minecraft:login_disconnect": {"protocol_id": 0}, "minecraft:login_finished": {"protocol_id": 2}},
        "serverbound": {"minecraft:hello": {"protocol_id": 0}, "minecraft:login_acknowledged": {"protocol_id": 3}}
    },
    "configuration": {
        "clientbound": {"minecraft:finish_configuration": {"protocol_id": 3}, "minecraft:post_effects": {"protocol_id": 10}, "minecraft:store_cookie": {"protocol_id": 11}},
        "serverbound": {"minecraft:finish_configuration": {"protocol_id": 3}}
    },
    "play": {
        "clientbound": {"minecraft:block_update": {"protocol_id": 9}, "minecraft:add_transient_block": {"protocol_id": 37}, "minecraft:keep_alive": {"protocol_id": 45}},
        "serverbound": {"minecraft:keep_alive": {"protocol_id": 28}, "minecraft:punch": {"protocol_id": 46}}
    }
}"#;

const LEGACY: &str = r#"{
    "login": {
        "clientbound": {"minecraft:login_disconnect": {"protocol_id": 0}, "minecraft:login_finished": {"protocol_id": 2}},
        "serverbound": {"minecraft:hello": {"protocol_id": 0}, "minecraft:login_acknowledged": {"protocol_id": 3}}
    },
    "configuration": {
        "clientbound": {"minecraft:finish_configuration": {"protocol_id": 3}, "minecraft:store_cookie": {"protocol_id": 10}},
        "serverbound": {"minecraft:finish_configuration": {"protocol_id": 3}}
    },
    "play": {
        "clientbound": {"minecraft:block_update": {"protocol_id": 9}, "minecraft:keep_alive": {"protocol_id": 44}},
        "serverbound": {"minecraft:keep_alive": {"protocol_id": 27}, "minecraft:swing": {"protocol_id": 63}}
    }
}"#;

fn session() -> Session {
    let canonical = Reports::parse(CANONICAL, "{}", "{}").unwrap();
    let legacy = Reports::parse(LEGACY, "{}", "{}").unwrap();
    test_session(Tables {
        packets: Packets::new(&canonical, &legacy),
        ..Tables::default()
    })
}

fn frame(id: i32, body: &[u8]) -> RawFrame {
    RawFrame {
        id,
        body: Bytes::copy_from_slice(body),
    }
}

#[test]
fn phases_follow_the_client_acknowledgements() {
    let mut s = session();
    // Login: the session UUID 26.3 appends to the profile is cut.
    let profile = [1u8; 20];
    let (id, body) = s.clientbound(2, &profile).unwrap().unwrap();
    assert_eq!((id, body.len()), (2, 4));
    assert!(s.serverbound(frame(3, &[])).unwrap().is_some());
    // Configuration: post_effects is dropped, later ids shift back by one.
    assert!(s.clientbound(10, &[0]).unwrap().is_none());
    assert_eq!(s.clientbound(11, &[7]).unwrap().unwrap().0, 10);
    assert_eq!(s.serverbound(frame(3, &[])).unwrap().unwrap().id, 3);
    // Play.
    assert_eq!(s.clientbound(45, &[0; 8]).unwrap().unwrap().0, 44);
    assert!(s.clientbound(37, &[0; 9]).unwrap().is_none());
    assert_eq!(s.serverbound(frame(27, &[0; 8])).unwrap().unwrap().id, 28);
    assert_eq!(
        s.serverbound(frame(63, &[0])).unwrap().unwrap().id,
        46,
        "legacy swings become the canonical empty punch"
    );
    assert!(
        s.serverbound(frame(99, &[])).is_err(),
        "unknown ids fail closed"
    );
}

#[test]
fn pre_framed_batches_are_reframed_with_compression() {
    let mut s = session();
    s.phase = Phase::Play;
    let compression = Compression::Threshold(16);
    let mut batch = BytesMut::new();
    mc_protocol::frame::append_frame(&mut batch, 45, &[1; 8], compression).unwrap();
    mc_protocol::frame::append_frame(&mut batch, 37, &[2; 40], compression).unwrap();
    mc_protocol::frame::append_frame(&mut batch, 45, &[3; 64], compression).unwrap();
    let mut out = s.clientbound_frames(&batch, compression).unwrap();
    let first = mc_protocol::frame::try_decode_frame(&mut out, compression)
        .unwrap()
        .unwrap();
    assert_eq!((first.id, &first.body[..]), (44, &[1u8; 8][..]));
    let second = mc_protocol::frame::try_decode_frame(&mut out, compression)
        .unwrap()
        .unwrap();
    assert_eq!((second.id, &second.body[..]), (44, &[3u8; 64][..]));
    assert!(out.is_empty(), "the 26.3-only frame is gone");
}

#[test]
fn malformed_world_updates_fail_instead_of_being_silently_lost() {
    let mut s = session();
    s.phase = Phase::Play;
    assert!(s.clientbound(9, &[]).is_err());
}

#[test]
fn legacy_cache_sits_beside_the_canonical_one() {
    if std::env::var_os(CACHE_ENV).is_some() {
        return;
    }
    let release = Path::new("/srv/content").join(mc_protocol::TARGET_RELEASE);
    assert_eq!(cache_root(&release), Path::new("/srv/content/26.1.2"));
    assert_eq!(
        cache_root(Path::new("data/vanilla")),
        Path::new("data/vanilla-26.1.2")
    );
}
