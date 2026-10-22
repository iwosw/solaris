use super::*;

#[test]
fn title_text_wire_id_and_component_round_trip() {
    assert_eq!(ClientboundSetTitleText::ID, 0x72);
    let packet = ClientboundSetTitleText {
        content: Tag::Compound(vec![("text".into(), Tag::String("Hello".into()))]),
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert!(!bytes.is_empty());
    let mut input = bytes.as_slice();
    assert_eq!(ClientboundSetTitleText::decode(&mut input).unwrap(), packet);
    assert!(input.is_empty());
}

#[test]
fn subtitle_and_action_bar_wire_ids_and_components_round_trip() {
    assert_eq!(ClientboundSetSubtitleText::ID, 0x70);
    assert_eq!(ClientboundSetActionBarText::ID, 0x57);
    let subtitle = ClientboundSetSubtitleText {
        content: Tag::String("S".into()),
    };
    let mut subtitle_bytes = Vec::new();
    subtitle.encode(&mut subtitle_bytes).unwrap();
    let mut input = subtitle_bytes.as_slice();
    assert_eq!(
        ClientboundSetSubtitleText::decode(&mut input).unwrap(),
        subtitle
    );
    assert!(input.is_empty());

    let action_bar = ClientboundSetActionBarText {
        content: Tag::Compound(vec![("text".into(), Tag::String("A".into()))]),
    };
    let mut action_bytes = Vec::new();
    action_bar.encode(&mut action_bytes).unwrap();
    let mut input = action_bytes.as_slice();
    assert_eq!(
        ClientboundSetActionBarText::decode(&mut input).unwrap(),
        action_bar
    );
    assert!(input.is_empty());
}

#[test]
fn titles_animation_writes_three_big_endian_ints() {
    assert_eq!(ClientboundSetTitlesAnimation::ID, 0x73);
    let packet = ClientboundSetTitlesAnimation {
        fade_in: 10,
        stay: 70,
        fade_out: 20,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes, vec![0, 0, 0, 10, 0, 0, 0, 70, 0, 0, 0, 20]);
    let mut input = bytes.as_slice();
    assert_eq!(
        ClientboundSetTitlesAnimation::decode(&mut input).unwrap(),
        packet
    );
    assert!(input.is_empty());
}

#[test]
fn clear_titles_reset_flag_round_trips() {
    assert_eq!(ClientboundClearTitles::ID, 0x0E);
    for reset_times in [false, true] {
        let packet = ClientboundClearTitles { reset_times };
        let mut bytes = Vec::new();
        packet.encode(&mut bytes).unwrap();
        assert_eq!(bytes, vec![u8::from(reset_times)]);
        let mut input = bytes.as_slice();
        assert_eq!(ClientboundClearTitles::decode(&mut input).unwrap(), packet);
        assert!(input.is_empty());
    }
}

#[test]
fn truncated_and_trailing_title_input_fails_or_leaves_remainder() {
    let packet = ClientboundSetTitlesAnimation {
        fade_in: 1,
        stay: 2,
        fade_out: 3,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut truncated = &bytes[..11];
    assert!(ClientboundSetTitlesAnimation::decode(&mut truncated).is_err());
    bytes.push(0xFF);
    let mut input = bytes.as_slice();
    assert_eq!(
        ClientboundSetTitlesAnimation::decode(&mut input).unwrap(),
        packet
    );
    assert_eq!(input, [0xFF]);

    let clear = ClientboundClearTitles { reset_times: true };
    let mut clear_bytes = Vec::new();
    clear.encode(&mut clear_bytes).unwrap();
    let mut empty: &[u8] = &[];
    assert!(ClientboundClearTitles::decode(&mut empty).is_err());

    let title = ClientboundSetTitleText {
        content: Tag::String("T".into()),
    };
    let mut title_bytes = Vec::new();
    title.encode(&mut title_bytes).unwrap();
    let mut empty: &[u8] = &[];
    assert!(ClientboundSetTitleText::decode(&mut empty).is_err());
    title_bytes.push(0x01);
    let mut input = title_bytes.as_slice();
    assert_eq!(ClientboundSetTitleText::decode(&mut input).unwrap(), title);
    assert_eq!(input, [0x01]);
}
