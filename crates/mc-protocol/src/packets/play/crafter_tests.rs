use super::*;
#[test]
fn crafter_slot_state_verified_packet_id_and_varint_field_order() {
    assert_eq!(ServerboundContainerSlotStateChanged::ID, 0x14);
    for enabled in [false, true] {
        let packet = ServerboundContainerSlotStateChanged {
            slot_id: 8,
            container_id: 130,
            new_state: enabled,
        };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(bytes, vec![8, 0x82, 1, u8::from(enabled)]);
        assert_eq!(
            ServerboundContainerSlotStateChanged::decode(&mut &bytes[..]).unwrap(),
            packet
        );
    }
}
#[test]
fn crafter_slot_state_rejects_truncated_payload() {
    for bytes in [&[][..], &[8][..], &[8, 1][..], &[8, 0x80][..]] {
        assert!(ServerboundContainerSlotStateChanged::decode(&mut &bytes[..]).is_err());
    }
}
