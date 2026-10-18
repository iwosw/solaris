use super::*;
use bytes::{Bytes, BytesMut};

#[test]
fn set_beacon_id_matches_vanilla_registration_index() {
    // Local 26.1.2 `javap GameProtocols.lambda$static$1`: SERVERBOUND_SET_BEACON
    // is the 53rd serverbound game addPacket call = wire id 0x34 (0x33 is
    // SERVERBOUND_SELECT_TRADE, 0x35 is SERVERBOUND_SET_CARRIED_ITEM, both
    // pinned by existing packets).
    assert_eq!(ServerboundSetBeacon::ID, 0x34);
}

#[test]
fn set_beacon_none_none_encodes_two_false_bytes() {
    let packet = ServerboundSetBeacon {
        primary: None,
        secondary: None,
    };
    let mut bytes = BytesMut::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes.freeze().as_ref(), &[0, 0]);
}

#[test]
fn set_beacon_roundtrip_preserves_zero_based_registry_ids() {
    // Zero-based registry ids: speed = 0, regeneration = 9.
    let packet = ServerboundSetBeacon {
        primary: Some(0),
        secondary: Some(9),
    };
    let mut buf = BytesMut::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf.as_ref(), &[1, 0, 1, 9]);
    assert_eq!(
        ServerboundSetBeacon::decode(&mut buf.freeze()).unwrap(),
        packet
    );
}

#[test]
fn set_beacon_mixed_present_absent_slots_roundtrip() {
    for packet in [
        ServerboundSetBeacon {
            primary: Some(4),
            secondary: None,
        },
        ServerboundSetBeacon {
            primary: None,
            secondary: Some(10),
        },
    ] {
        let mut buf = BytesMut::new();
        packet.encode(&mut buf).unwrap();
        assert_eq!(
            ServerboundSetBeacon::decode(&mut buf.freeze()).unwrap(),
            packet
        );
    }
}

#[test]
fn set_beacon_rejects_truncated_and_unknown_ids() {
    assert!(ServerboundSetBeacon::decode(&mut Bytes::from_static(&[1])).is_err());
    assert!(ServerboundSetBeacon::decode(&mut Bytes::from_static(&[1, 1])).is_err());
    // Unknown vanilla effect registry id.
    let mut buf = BytesMut::new();
    buf.write_bool(true);
    buf.write_varint(500);
    assert!(ServerboundSetBeacon::decode(&mut buf.freeze()).is_err());
    // Negative VarInt id.
    let mut buf = BytesMut::new();
    buf.write_bool(true);
    buf.write_varint(-1);
    assert!(ServerboundSetBeacon::decode(&mut buf.freeze()).is_err());
    // Encode refuses unknown ids too.
    let mut buf = BytesMut::new();
    assert!(
        ServerboundSetBeacon {
            primary: Some(5000),
            secondary: None,
        }
        .encode(&mut buf)
        .is_err()
    );
}
