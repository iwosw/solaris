//! Tests for [`crate::connection`]: budgets, guards, helpers.

use bytes::BytesMut;
use mc_protocol::frame::{Compression, RawFrame, encode_frame};

use super::{
    MAX_INBOUND_BUFFER_BYTES, MAX_PRE_PLAY_BYTES, MAX_PRE_PLAY_PACKETS, PrePlayBudget,
    text_component_nbt,
};

#[test]
fn budget_accepts_within_limits() {
    let mut budget = PrePlayBudget::new(MAX_PRE_PLAY_PACKETS, MAX_PRE_PLAY_BYTES);
    let frame = RawFrame {
        id: 0,
        body: bytes::Bytes::from(vec![0u8; 64]),
    };
    for _ in 0..MAX_PRE_PLAY_PACKETS {
        budget.record(&frame).unwrap();
    }
    assert!(budget.record(&frame).is_err(), "packet budget must trip");
}

#[test]
fn budget_rejects_byte_overflow() {
    let mut budget = PrePlayBudget::new(10_000, 100);
    let frame = RawFrame {
        id: 0,
        body: bytes::Bytes::from(vec![0u8; 200]),
    };
    assert!(budget.record(&frame).is_err());
}

#[test]
fn text_component_nbt_round_trips() {
    let bytes = text_component_nbt("hello").unwrap();
    let tag = mc_nbt::read_network(&mut bytes.as_slice()).unwrap();
    assert_eq!(
        tag,
        mc_nbt::Tag::Compound(vec![(
            "text".to_owned(),
            mc_nbt::Tag::String("hello".to_owned()),
        )])
    );
}

#[test]
fn framed_packets_decode_uncompressed() {
    let framed = encode_frame(0x03, b"payload", Compression::Disabled).unwrap();
    let mut buf = BytesMut::from(framed.as_ref());
    let frame = mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
        .unwrap()
        .expect("complete frame");
    assert_eq!(frame.id, 0x03);
    assert_eq!(frame.body.as_ref(), b"payload");
}

#[test]
fn incomplete_frame_waits_for_more_bytes() {
    let framed = encode_frame(0x01, b"abc", Compression::Disabled).unwrap();
    let mut buf = BytesMut::from(&framed[..2]);
    assert!(
        mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .is_none()
    );
    buf.extend_from_slice(&framed[2..]);
    assert!(
        mc_protocol::frame::try_decode_frame(&mut buf, Compression::Disabled)
            .unwrap()
            .is_some()
    );
}

#[test]
fn inbound_cap_is_sane() {
    assert_eq!(MAX_INBOUND_BUFFER_BYTES, 1024 * 1024);
}
