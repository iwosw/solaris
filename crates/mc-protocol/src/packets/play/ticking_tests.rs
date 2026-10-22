use super::*;

#[test]
fn ticking_state_wire_id_layout_and_round_trip() {
    assert_eq!(ClientboundTickingState::ID, 0x7F);
    for (tick_rate, frozen) in [(20.0_f32, false), (4.0_f32, true)] {
        let packet = ClientboundTickingState { tick_rate, frozen };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(bytes.len(), 5);
        assert_eq!(&bytes[..4], &tick_rate.to_be_bytes());
        assert_eq!(bytes[4], u8::from(frozen));
        let mut input = bytes.as_slice();
        assert_eq!(ClientboundTickingState::decode(&mut input).unwrap(), packet);
        assert!(input.is_empty());
    }
}

#[test]
fn ticking_step_wire_id_uses_varint() {
    assert_eq!(ClientboundTickingStep::ID, 0x80);
    for tick_steps in [0, 1, 100, 300] {
        let packet = ClientboundTickingStep { tick_steps };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        let mut input = bytes.as_slice();
        assert_eq!(ClientboundTickingStep::decode(&mut input).unwrap(), packet);
        assert!(input.is_empty());
    }
    // Multi-byte VarInt: 300 encodes as [0xAC, 0x02].
    let packet = ClientboundTickingStep { tick_steps: 300 };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes, vec![0xAC, 0x02]);
}

#[test]
fn truncated_and_trailing_ticking_input() {
    let packet = ClientboundTickingState {
        tick_rate: 20.0,
        frozen: false,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut empty: &[u8] = &[];
    assert!(ClientboundTickingState::decode(&mut empty).is_err());
    let mut truncated = &bytes[..4];
    assert!(ClientboundTickingState::decode(&mut truncated).is_err());
    bytes.push(0xFF);
    let mut input = bytes.as_slice();
    assert_eq!(ClientboundTickingState::decode(&mut input).unwrap(), packet);
    assert_eq!(input, [0xFF]);

    let mut empty: &[u8] = &[];
    assert!(ClientboundTickingStep::decode(&mut empty).is_err());
}
