use super::*;
use bytes::BytesMut;
#[test]
fn edit_book_wire_and_bounds_match_local_codec() {
    let packet = ServerboundEditBook {
        slot: 40,
        pages: vec!["one".into()],
        title: Some("title".into()),
    };
    assert_eq!(ServerboundEditBook::ID, 24);
    let mut bytes = BytesMut::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes.as_ref(), b"\x28\x01\x03one\x01\x05title");
    assert_eq!(
        ServerboundEditBook::decode(&mut bytes.freeze()).unwrap(),
        packet
    );
    assert!(ServerboundEditBook::decode(&mut &b"\x00\x65"[..]).is_err());
    let mut bad = packet;
    bad.title = Some("x".repeat(33));
    let mut bytes = BytesMut::new();
    assert!(bad.encode(&mut bytes).is_err());
    assert!(bytes.is_empty());
}
#[test]
fn open_book_carries_only_hand_enum() {
    assert_eq!(ClientboundOpenBook::ID, 58);
    let packet = ClientboundOpenBook {
        hand: InteractionHand::OffHand,
    };
    let mut bytes = BytesMut::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes.as_ref(), &[1]);
    assert_eq!(
        ClientboundOpenBook::decode(&mut bytes.freeze()).unwrap(),
        packet
    );
}
