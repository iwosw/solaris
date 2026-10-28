use super::*;

fn id(value: &str) -> Identifier {
    Identifier::parse(value).unwrap()
}

#[test]
fn vanilla_registration_ids_and_empty_reset_layout() {
    // Local 26.1.2 GameProtocols javap registration ordinals.
    assert_eq!(ClientboundUpdateAdvancements::ID, 0x85);
    assert_eq!(ClientboundSelectAdvancementsTab::ID, 0x57);
    assert_eq!(ServerboundSeenAdvancements::ID, 0x33);
    let packet = ClientboundUpdateAdvancements {
        reset: true,
        added: Vec::new(),
        removed: Vec::new(),
        progress: BTreeMap::new(),
        show_advancements: true,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    assert_eq!(bytes, [1, 0, 0, 0, 1]);
}

#[test]
fn delta_criteria_use_nullable_signed_epoch_millisecond_longs() {
    // AdvancementProgress.writeMap + CriterionProgress.writeNullable(writeInstant).
    let packet = ClientboundUpdateAdvancements {
        reset: false,
        added: Vec::new(),
        removed: vec![id("a:b")],
        progress: BTreeMap::from([(
            id("a:c"),
            BTreeMap::from([("x".into(), Some(-2)), ("y".into(), None)]),
        )]),
        show_advancements: false,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut expected = vec![
        0, 0, 1, 3, b'a', b':', b'b', 1, 3, b'a', b':', b'c', 2, 1, b'x', 1,
    ];
    expected.extend_from_slice(&(-2_i64).to_be_bytes());
    expected.extend_from_slice(&[1, b'y', 0, 0]);
    assert_eq!(bytes, expected);
    let mut input = bytes.as_slice();
    assert_eq!(
        ClientboundUpdateAdvancements::decode(&mut input).unwrap(),
        packet
    );
    assert!(input.is_empty());
}

#[test]
fn display_template_flags_requirements_and_telemetry_follow_javap_order() {
    let items = mc_data::items::solaris_required_items();
    let stone = items.id_of(&id("minecraft:stone")).unwrap();
    let packet = ClientboundUpdateAdvancements {
        reset: true,
        added: vec![AdvancementDefinition {
            id: id("a:b"),
            parent: Some(id("a:p")),
            display: Some(AdvancementDisplay {
                title: Tag::String("T".into()),
                description: Tag::String("D".into()),
                icon: mc_data::ItemStack::new(stone, 1),
                frame: AdvancementFrame::Challenge,
                background: Some(id("a:g")),
                show_toast: true,
                hidden: true,
                x: 2.0,
                y: -1.0,
            }),
            requirements: vec![vec!["x".into(), "y".into()], vec!["z".into()]],
            sends_telemetry: true,
        }],
        removed: Vec::new(),
        progress: BTreeMap::new(),
        show_advancements: true,
    };
    let mut bytes = Vec::new();
    packet.encode(&mut bytes).unwrap();
    let mut input = bytes.as_slice();
    assert!(input.read_bool().unwrap());
    assert_eq!(input.read_varint().unwrap(), 1);
    assert_eq!(input.read_identifier().unwrap(), id("a:b"));
    assert!(input.read_bool().unwrap());
    assert_eq!(input.read_identifier().unwrap(), id("a:p"));
    assert!(input.read_bool().unwrap());
    assert_eq!(
        mc_nbt::read_network_value(&mut input).unwrap(),
        Tag::String("T".into())
    );
    assert_eq!(
        mc_nbt::read_network_value(&mut input).unwrap(),
        Tag::String("D".into())
    );
    assert_eq!(input.read_varint().unwrap(), stone as i32);
    assert_eq!(input.read_varint().unwrap(), 1);
    assert_eq!(input.read_varint().unwrap(), 0); // added component count
    assert_eq!(input.read_varint().unwrap(), 0); // removed component count
    assert_eq!(input.read_varint().unwrap(), 1); // CHALLENGE ordinal
    assert_eq!(input.read_i32().unwrap(), 7);
    assert_eq!(input.read_identifier().unwrap(), id("a:g"));
    // 26.3 PositionedAdvancement: x and y follow the whole holder.
    let (requirements, rest) = input.split_at(10);
    assert_eq!(requirements, [2, 2, 1, b'x', 1, b'y', 1, 1, b'z', 1]);
    input = rest;
    assert_eq!(input.read_f32().unwrap(), 2.0);
    assert_eq!(input.read_f32().unwrap(), -1.0);
    assert_eq!(input, &[0, 0, 1]);
    assert_eq!(
        ClientboundUpdateAdvancements::decode(&mut bytes.as_slice()).unwrap(),
        packet
    );
}

#[test]
fn tab_actions_and_optional_selected_tab_layout() {
    let mut bytes = Vec::new();
    ServerboundSeenAdvancements::OpenedTab(id("a:b"))
        .encode(&mut bytes)
        .unwrap();
    assert_eq!(bytes, [0, 3, b'a', b':', b'b']);
    bytes.clear();
    ServerboundSeenAdvancements::ClosedScreen
        .encode(&mut bytes)
        .unwrap();
    assert_eq!(bytes, [1]);
    assert!(ServerboundSeenAdvancements::decode(&mut &[2][..]).is_err());
    assert!(ServerboundSeenAdvancements::decode(&mut &[0][..]).is_err());
    bytes.clear();
    ClientboundSelectAdvancementsTab {
        tab: Some(id("a:b")),
    }
    .encode(&mut bytes)
    .unwrap();
    assert_eq!(bytes, [1, 3, b'a', b':', b'b']);
    bytes.clear();
    ClientboundSelectAdvancementsTab { tab: None }
        .encode(&mut bytes)
        .unwrap();
    assert_eq!(bytes, [0]);
}
