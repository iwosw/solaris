use super::*;
use crate::packets::login::GameProfileProperty;
use crate::packets::{ChatVisibility, MainHand, ParticleStatus, ResourcePackAction};

fn round_trip<P: Packet + PartialEq + std::fmt::Debug>(p: P) {
    let mut buf = Vec::new();
    p.encode(&mut buf).unwrap();
    let mut cursor: &[u8] = &buf;
    let decoded: P = P::decode(&mut cursor).unwrap();
    assert_eq!(decoded, p);
    assert!(cursor.is_empty(), "all bytes consumed");
}

fn sample_identifier(s: &str) -> Identifier {
    Identifier::parse(s).unwrap()
}

#[test]
fn login_play_round_trip_minimum() {
    round_trip(LoginPlay {
        entity_id: 1,
        is_hardcore: false,
        dimension_names: vec![sample_identifier("minecraft:overworld")],
        max_players: 20,
        view_distance: 10,
        simulation_distance: 10,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id: 0,
        dimension_name: sample_identifier("minecraft:overworld"),
        hashed_seed: 0,
        game_mode: 0,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        enforces_secure_chat: false,
        online_mode: true,
    });
}

#[test]
fn login_play_round_trip_with_death_location() {
    round_trip(LoginPlay {
        entity_id: 42,
        is_hardcore: true,
        dimension_names: vec![
            sample_identifier("minecraft:overworld"),
            sample_identifier("minecraft:the_nether"),
            sample_identifier("minecraft:the_end"),
        ],
        max_players: 100,
        view_distance: 16,
        simulation_distance: 12,
        reduced_debug_info: true,
        enable_respawn_screen: false,
        do_limited_crafting: true,
        dimension_type_id: 2,
        dimension_name: sample_identifier("minecraft:the_nether"),
        hashed_seed: i64::MIN,
        game_mode: 3,
        previous_game_mode: 0,
        is_debug: true,
        is_flat: true,
        death_location: Some((sample_identifier("minecraft:overworld"), 1_234_567_890)),
        portal_cooldown: 100,
        sea_level: 0,
        enforces_secure_chat: true,
        online_mode: false,
    });
}

#[test]
fn login_play_rejects_infeasible_dimension_count_before_decode() {
    let mut body = Vec::new();
    body.write_i32(1);
    body.write_bool(false);
    body.write_varint(MAX_DIMENSION_NAMES as i32);

    assert_eq!(
        LoginPlay::decode(&mut body.as_slice()).unwrap_err(),
        CodecError::Underflow {
            needed: MAX_DIMENSION_NAMES,
            available: 0,
        }
    );
}

#[test]
fn login_play_encode_rejects_dimension_count_over_decode_cap() {
    let packet = LoginPlay {
        entity_id: 1,
        is_hardcore: false,
        dimension_names: vec![sample_identifier("minecraft:overworld"); MAX_DIMENSION_NAMES + 1],
        max_players: 20,
        view_distance: 10,
        simulation_distance: 10,
        reduced_debug_info: false,
        enable_respawn_screen: true,
        do_limited_crafting: false,
        dimension_type_id: 0,
        dimension_name: sample_identifier("minecraft:overworld"),
        hashed_seed: 0,
        game_mode: 0,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        enforces_secure_chat: false,
        online_mode: true,
    };
    let mut body = Vec::new();

    assert_eq!(
        packet.encode(&mut body).unwrap_err(),
        CodecError::StringTooLong {
            len: MAX_DIMENSION_NAMES + 1,
            max: MAX_DIMENSION_NAMES,
        }
    );
    assert!(body.is_empty());
}

#[test]
fn clientbound_custom_payload_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ClientboundCustomPayload::ID, 0x18);
    let packet = ClientboundCustomPayload {
        payload: CustomPayload::Unknown {
            channel: Identifier::parse("solaris:test").unwrap(),
            payload: vec![0xDE, 0xAD, 0xBE, 0xEF],
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x0c, b's', b'o', b'l', b'a', b'r', b'i', b's', b':', b't', b'e', b's', b't', 0xDE,
            0xAD, 0xBE, 0xEF,
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundCustomPayload::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_respawn_id_and_layout_match_javap() {
    assert_eq!(ClientboundRespawn::ID, 0x54);
    let packet = ClientboundRespawn {
        dimension_type_id: 0,
        dimension_name: sample_identifier("overworld"),
        hashed_seed: 0,
        game_mode: 0,
        previous_game_mode: -1,
        is_debug: false,
        is_flat: false,
        death_location: None,
        portal_cooldown: 0,
        sea_level: 63,
        data_to_keep: 0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, // dimension type id
            0x13, b'm', b'i', b'n', b'e', b'c', b'r', b'a', b'f', b't', b':', b'o', b'v', b'e',
            b'r', b'w', b'o', b'r', b'l', b'd', 0, 0, 0, 0, 0, 0, 0, 0, 0,    // game mode
            0x00, // previous game mode: optional varint, 0 = none
            0, 0,  // debug/flat
            0,  // no death location
            0,  // portal cooldown
            63, // sea level
            0,  // data to keep
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundRespawn::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn set_default_spawn_round_trip() {
    round_trip(SetDefaultSpawnPosition {
        dimension: Identifier::parse("minecraft:overworld").unwrap(),
        position: 0x0000_0FFF_FFFF_FFFF,
        yaw: 1.5,
        pitch: -2.0,
    });
}

#[test]
fn block_entity_data_id_and_layout_match_protocol_dump() {
    assert_eq!(ClientboundBlockEntityData::ID, 0x06);
    let packet = ClientboundBlockEntityData {
        position: pack_block_pos(1, 2, 3),
        block_entity_type: 7,
        nbt: Tag::Compound(vec![("id".into(), Tag::String("minecraft:sign".into()))]),
    };

    round_trip(packet);
}

#[test]
fn block_event_id_and_layout_match_local_vanilla_codec() {
    assert_eq!(ClientboundBlockEvent::ID, 0x07);
    let packet = ClientboundBlockEvent {
        position: pack_block_pos(-1, 64, 2),
        action: 1,
        parameter: 2,
        block_type: 300,
    };
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();

    let mut expected = Vec::new();
    expected.write_i64(packet.position);
    expected.write_u8(1);
    expected.write_u8(2);
    expected.write_varint(300);
    assert_eq!(body, expected);

    let mut cursor: &[u8] = &body;
    assert_eq!(ClientboundBlockEvent::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn open_sign_editor_id_and_layout_match_protocol_dump() {
    assert_eq!(ClientboundOpenSignEditor::ID, 0x3D);
    let packet = ClientboundOpenSignEditor {
        position: pack_block_pos(-1, 64, 2),
        is_front_text: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();

    let mut expected = Vec::new();
    expected.write_i64(packet.position);
    expected.write_bool(true);
    assert_eq!(buf, expected);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundOpenSignEditor::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn block_destruction_id_and_layout_match_local_vanilla_codec() {
    assert_eq!(ClientboundBlockDestruction::ID, 0x05);
    let packet = ClientboundBlockDestruction {
        breaker_entity_id: 300,
        position: pack_block_pos(10, 64, 20),
        progress: 7,
    };
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();

    let mut expected = Vec::new();
    expected.write_varint(300);
    expected.write_i64(packet.position);
    expected.write_u8(7);
    assert_eq!(body, expected);

    let mut cursor: &[u8] = &body;
    assert_eq!(
        ClientboundBlockDestruction::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_sign_update_id_and_layout_match_vanilla_stream_codec() {
    assert_eq!(ServerboundSignUpdate::ID, 0x3E);
    let packet = ServerboundSignUpdate {
        position: pack_block_pos(1, 65, -2),
        lines: vec!["one".into(), "two".into(), "three".into(), "four".into()],
        is_front_text: false,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();

    // 26.3: position, four lines, then the SignTextSlot varint (0 = back).
    let mut expected = Vec::new();
    expected.write_i64(packet.position);
    expected.write_string("one", MAX_SIGN_LINE_LEN).unwrap();
    expected.write_string("two", MAX_SIGN_LINE_LEN).unwrap();
    expected.write_string("three", MAX_SIGN_LINE_LEN).unwrap();
    expected.write_string("four", MAX_SIGN_LINE_LEN).unwrap();
    expected.write_varint(0);
    assert_eq!(buf, expected);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundSignUpdate::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_sign_update_rejects_overlong_vanilla_line() {
    let packet = ServerboundSignUpdate {
        position: pack_block_pos(1, 65, -2),
        lines: vec![
            "x".repeat(MAX_SIGN_LINE_LEN + 1),
            "".into(),
            "".into(),
            "".into(),
        ],
        is_front_text: true,
    };

    assert!(packet.encode(&mut Vec::new()).is_err());
}

#[test]
fn synchronize_player_position_round_trip() {
    round_trip(SynchronizePlayerPosition {
        teleport_id: 1,
        x: 0.5,
        y: 64.0,
        z: -0.5,
        dx: 0.0,
        dy: 0.0,
        dz: 0.0,
        yaw: 0.0,
        pitch: 0.0,
        relative_flags: 0,
    });
}

#[test]
fn keepalive_round_trip_both_directions() {
    round_trip(ClientboundKeepAlive {
        id: 0x0123_4567_89AB_CDEF,
    });
    round_trip(ServerboundKeepAlive { id: i64::MIN });
    round_trip(ServerboundKeepAlive { id: 0 });
}

#[test]
fn move_player_ids_match_javap() {
    assert_eq!(ServerboundMovePlayerPos::ID, 0x1E);
    assert_eq!(ServerboundMovePlayerPosRot::ID, 0x1F);
    assert_eq!(ServerboundMovePlayerRot::ID, 0x20);
    assert_eq!(ServerboundMovePlayerStatusOnly::ID, 0x21);
}

#[test]
fn move_player_packets_round_trip() {
    let flags = MovePlayerFlags::new(true, true);
    round_trip(ServerboundMovePlayerPos {
        x: 16.5,
        y: -58.0,
        z: -0.25,
        flags,
    });
    round_trip(ServerboundMovePlayerPosRot {
        x: -16.5,
        y: 70.0,
        z: 32.25,
        yaw: 180.0,
        pitch: -20.0,
        flags,
    });
    round_trip(ServerboundMovePlayerRot {
        yaw: 45.0,
        pitch: 10.0,
        flags,
    });
    round_trip(ServerboundMovePlayerStatusOnly { flags });
}

#[test]
fn move_player_flags_use_low_two_bits() {
    let packet = ServerboundMovePlayerStatusOnly {
        flags: MovePlayerFlags::new(true, true),
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x03]);

    let mut cursor: &[u8] = &[0x02];
    let decoded = ServerboundMovePlayerStatusOnly::decode(&mut cursor).unwrap();
    assert_eq!(decoded.flags, MovePlayerFlags::new(false, true));
}

#[test]
fn confirm_teleportation_round_trip() {
    round_trip(ConfirmTeleportation {
        teleport_id: 1,
        position: [1.5, 64.0, -2.25],
        yaw: 90.0,
        pitch: -10.0,
    });
}

#[test]
fn play_disconnect_carries_opaque_nbt_bytes() {
    // We are not parsing NBT inside the packet, just shuttling it.
    round_trip(PlayDisconnect {
        reason_nbt: vec![0x0A, 0x00, 0x08, b'r', b'e', b'a', b's', b'o', b'n', 0x00],
    });
}

#[test]
fn forget_level_chunk_id_matches_javap() {
    assert_eq!(ForgetLevelChunk::ID, 0x26);
}

#[test]
fn forget_level_chunk_round_trips() {
    for (x, z) in [(0, 0), (1, -1), (-100_000, 100_000), (i32::MIN, i32::MAX)] {
        round_trip(ForgetLevelChunk {
            chunk_x: x,
            chunk_z: z,
        });
    }
}

#[test]
fn forget_level_chunk_wire_layout_uses_chunk_pos_long() {
    let packet = ForgetLevelChunk {
        chunk_x: -1,
        chunk_z: 2,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0, 0, 0, 2, 0xFF, 0xFF, 0xFF, 0xFF]);
}

#[test]
fn game_event_round_trip() {
    round_trip(GameEvent {
        event: GameEvent::EVENT_START_WAITING_FOR_CHUNKS,
        value: 0.0,
    });
}

#[test]
fn initialize_border_id_and_layout_match_javap() {
    assert_eq!(ClientboundInitializeBorder::ID, 0x2C);
    let packet = ClientboundInitializeBorder {
        center_x: 0.0,
        center_z: 0.0,
        old_size: 59_999_968.0,
        new_size: 59_999_968.0,
        lerp_time: 0,
        absolute_max_size: 29_999_984,
        warning_blocks: 5,
        warning_time: 15,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[0..8], &0.0_f64.to_be_bytes());
    assert_eq!(&buf[8..16], &0.0_f64.to_be_bytes());
    assert_eq!(&buf[16..24], &59_999_968.0_f64.to_be_bytes());
    assert_eq!(&buf[24..32], &59_999_968.0_f64.to_be_bytes());

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundInitializeBorder::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn player_visible_packet_ids_match_javap() {
    assert_eq!(AddEntity::ID, 0x01);
    assert_eq!(EntityPositionSync::ID, 0x23);
    assert_eq!(PlayerInfoRemove::ID, 0x46);
    assert_eq!(PlayerInfoUpdate::ID, 0x47);
    assert_eq!(RemoveEntities::ID, 0x4E);
    assert_eq!(RotateHead::ID, 0x55);
    assert_eq!(ClientboundSetEntityData::ID, 0x65);
    assert_eq!(ClientboundTakeItemEntity::ID, 0x7F);
    assert_eq!(ClientboundTabList::ID, 0x7D);
}

#[test]
fn player_info_update_minimal_add_player_wire_layout() {
    let uuid = Uuid::from_u128(0x00112233445566778899aabbccddeeff);
    let packet = PlayerInfoUpdate {
        actions: PlayerInfoActions::minimal_add_player(),
        entries: vec![PlayerInfoEntry {
            profile_id: uuid,
            name: "Steve".to_string(),
            properties: Vec::new(),
            listed: true,
            latency: 0,
            game_mode: 0,
            list_order: 0,
            show_hat: true,
        }],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 0xFD);
    assert_eq!(buf[1], 0x01);
    assert_eq!(&buf[2..18], uuid.as_bytes());
    assert_eq!(&buf[18..24], &[5, b'S', b't', b'e', b'v', b'e']);
    assert_eq!(&buf[24..], &[0, 0, 1, 0, 0, 0, 1]);

    let mut cursor: &[u8] = &buf;
    let decoded = PlayerInfoUpdate::decode(&mut cursor).unwrap();
    assert_eq!(decoded, packet);
    assert!(cursor.is_empty());
}

#[test]
fn player_info_add_player_properties_match_vanilla_wire_layout() {
    let uuid = Uuid::from_u128(0x00112233445566778899aabbccddeeff);
    let packet = PlayerInfoUpdate {
        actions: PlayerInfoActions::ADD_PLAYER,
        entries: vec![PlayerInfoEntry {
            profile_id: uuid,
            name: "Alex".to_string(),
            properties: vec![
                GameProfileProperty {
                    name: "textures".to_string(),
                    value: "signed-value".to_string(),
                    signature: Some("sig".to_string()),
                },
                GameProfileProperty {
                    name: "rank".to_string(),
                    value: "builder".to_string(),
                    signature: None,
                },
            ],
            listed: false,
            latency: 0,
            game_mode: 0,
            list_order: 0,
            show_hat: false,
        }],
    };

    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();

    let mut expected = vec![PlayerInfoActions::ADD_PLAYER.0, 1];
    expected.extend_from_slice(uuid.as_bytes());
    expected.extend_from_slice(&[4, b'A', b'l', b'e', b'x', 2]);
    expected.extend_from_slice(b"\x08textures\x0csigned-value\x01\x03sig");
    expected.extend_from_slice(b"\x04rank\x07builder\x00");
    assert_eq!(buf, expected);

    let mut cursor = buf.as_slice();
    assert_eq!(PlayerInfoUpdate::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

fn player_info_add_player_prefix(property_count: i32) -> Vec<u8> {
    let mut buf = vec![PlayerInfoActions::ADD_PLAYER.0, 1];
    buf.extend_from_slice(Uuid::nil().as_bytes());
    buf.write_string("Alex", 16).unwrap();
    buf.write_varint(property_count);
    buf
}

#[test]
fn player_info_add_player_rejects_negative_property_count() {
    let buf = player_info_add_player_prefix(-1);
    assert_eq!(
        PlayerInfoUpdate::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::NegativeLength(-1)
    );
}

#[test]
fn player_info_add_player_rejects_more_than_sixteen_properties() {
    let buf = player_info_add_player_prefix(17);
    assert_eq!(
        PlayerInfoUpdate::decode(&mut buf.as_slice()).unwrap_err(),
        CodecError::StringTooLong { len: 17, max: 16 }
    );
}

#[test]
fn player_info_add_player_enforces_property_string_limits() {
    let mut long_name = player_info_add_player_prefix(1);
    long_name.write_string(&"n".repeat(65), 65).unwrap();
    assert!(matches!(
        PlayerInfoUpdate::decode(&mut long_name.as_slice()),
        Err(CodecError::StringTooLong { max: 64, .. })
    ));

    let mut long_signature = player_info_add_player_prefix(1);
    long_signature.write_string("textures", 64).unwrap();
    long_signature.write_string("value", 32_767).unwrap();
    long_signature.write_bool(true);
    long_signature
        .write_string(&"s".repeat(1025), 1025)
        .unwrap();
    assert!(matches!(
        PlayerInfoUpdate::decode(&mut long_signature.as_slice()),
        Err(CodecError::StringTooLong { max: 1024, .. })
    ));
}

#[test]
fn add_player_entity_zero_motion_round_trips() {
    round_trip(AddEntity {
        entity_id: 42,
        uuid: Uuid::from_u128(0x00112233445566778899aabbccddeeff),
        entity_type_id: 155,
        x: 0.5,
        y: -59.0,
        z: 0.5,
        movement: EntityVec3::ZERO,
        pitch: 0.0,
        yaw: 0.0,
        head_yaw: 0.0,
        data: 0,
    });
}

#[test]
fn entity_position_sync_round_trips() {
    round_trip(EntityPositionSync {
        entity_id: 42,
        position: EntityVec3 {
            x: 1.0,
            y: 2.0,
            z: 3.0,
        },
        yaw: 90.0,
        pitch: -15.0,
        on_ground: true,
    });
}

#[test]
fn set_entity_data_item_stack_layout_matches_javap() {
    let packet = ClientboundSetEntityData {
        entity_id: 300,
        values: vec![EntityDataValue::ItemStack {
            index: ITEM_ENTITY_DATA_ITEM_INDEX,
            stack: ItemStack::new(5, 1),
        }],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0xAC,
            0x02,
            ITEM_ENTITY_DATA_ITEM_INDEX,
            ENTITY_DATA_ITEM_STACK_SERIALIZER_ID as u8,
            0x01,
            0x05,
            0x00,
            0x00,
            0xFF,
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_entity_data_baby_boolean_layout_matches_vanilla_26_1_2() {
    assert_eq!(ENTITY_DATA_BOOLEAN_SERIALIZER_ID, 8);
    let packet = ClientboundSetEntityData {
        entity_id: 301,
        values: vec![EntityDataValue::Boolean {
            index: 16,
            value: true,
        }],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAD, 0x02, 16, 8, 1, 0xFF]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_entity_data_sheep_wool_layout_matches_vanilla_26_1_2() {
    assert_eq!(ENTITY_DATA_BYTE_SERIALIZER_ID, 0);
    let packet = ClientboundSetEntityData {
        entity_id: 301,
        values: vec![EntityDataValue::Byte {
            index: SHEEP_ENTITY_DATA_WOOL_INDEX,
            value: 0x10,
        }],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAD, 0x02, 18, 0, 0x10, 0xFF]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_entity_data_player_pose_layout_matches_javap() {
    let packet = ClientboundSetEntityData {
        entity_id: 7,
        values: vec![
            EntityDataValue::Byte {
                index: ENTITY_DATA_SHARED_FLAGS_INDEX,
                value: 0x0A,
            },
            EntityDataValue::Pose {
                index: ENTITY_DATA_POSE_INDEX,
                pose: EntityPose::Swimming,
            },
        ],
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![7, 0, 0, 0x0A, 6, 20, 3, 0xFF]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetEntityData::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn take_item_entity_layout_matches_javap() {
    let packet = ClientboundTakeItemEntity {
        item_entity_id: 300,
        player_entity_id: 2,
        amount: 1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02, 0x02, 0x01]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundTakeItemEntity::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_punch_layout_matches_packets_report() {
    assert_eq!(ServerboundPunch::ID, 0x2E);
    let mut buf = Vec::new();
    ServerboundPunch.encode(&mut buf).unwrap();
    assert!(buf.is_empty());

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundPunch::decode(&mut cursor).unwrap(),
        ServerboundPunch
    );
}

#[test]
fn entity_animation_id_and_wire_layout_match_server_javap() {
    assert_eq!(EntityAnimation::ID, 0x02);
    let packet = EntityAnimation {
        entity_id: 300,
        action: EntityAnimationAction::CriticalHit,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02, 0x01]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(EntityAnimation::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn swing_animation_id_and_wire_layout_match_packets_report() {
    assert_eq!(ClientboundSwingAnimation::ID, 0x7B);
    let packet = ClientboundSwingAnimation::default_swing(300, true);
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02, 0x01, 0x01, 0x06]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSwingAnimation::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn entity_event_id_and_wire_layout_match_server_javap() {
    assert_eq!(EntityEvent::ID, 0x22);
    let packet = EntityEvent {
        entity_id: 0x0102_0304,
        event_id: -1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x01, 0x02, 0x03, 0x04, 0xFF]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(EntityEvent::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn move_entity_pos_id_and_wire_layout_match_server_javap() {
    assert_eq!(MoveEntityPos::ID, 0x36);
    let packet = MoveEntityPos {
        entity_id: 300,
        delta_x: 4,
        delta_y: -8,
        delta_z: 12,
        on_ground: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    // 26.3: varint props (onGround | stepCount << 1) precede the deltas.
    assert_eq!(
        buf,
        vec![0xAC, 0x02, 0x01, 0x00, 0x04, 0xFF, 0xF8, 0x00, 0x0C]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(MoveEntityPos::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn move_entity_pos_rot_id_and_wire_layout_match_server_javap() {
    assert_eq!(MoveEntityPosRot::ID, 0x37);
    let packet = MoveEntityPosRot {
        entity_id: 300,
        delta_x: 4,
        delta_y: -8,
        delta_z: 12,
        yaw: 64,
        pitch: 250,
        on_ground: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0xAC, 0x02, 0x01, 0x00, 0x04, 0xFF, 0xF8, 0x00, 0x0C, 0x40, 0xFA
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(MoveEntityPosRot::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn move_entity_rot_id_and_wire_layout_match_server_javap() {
    assert_eq!(MoveEntityRot::ID, 0x39);
    let packet = MoveEntityRot {
        entity_id: 300,
        yaw: 64,
        pitch: 250,
        on_ground: true,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    // 26.3: onGround precedes yaw and pitch.
    assert_eq!(buf, vec![0xAC, 0x02, 0x01, 0x40, 0xFA]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(MoveEntityRot::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn move_entity_delta_scales_to_vanilla_short_units() {
    assert_eq!(MoveEntityPosRot::delta_to_short(1.0 / 4096.0), 1);
    assert_eq!(MoveEntityPosRot::delta_to_short(-0.5), -2048);
    assert_eq!(MoveEntityPosRot::pack_degrees(90.0), 64);
}

#[test]
fn set_entity_motion_id_and_zero_motion_layout_match_server_javap() {
    assert_eq!(SetEntityMotion::ID, 0x67);
    let packet = SetEntityMotion {
        entity_id: 5,
        movement: EntityVec3::ZERO,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x05, 0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(SetEntityMotion::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn set_entity_motion_round_trips_non_zero_lp_vec3() {
    let packet = SetEntityMotion {
        entity_id: 42,
        movement: EntityVec3 {
            x: 0.1,
            y: -0.2,
            z: 0.3,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    let mut cursor: &[u8] = &buf;
    let decoded = SetEntityMotion::decode(&mut cursor).unwrap();
    assert_eq!(decoded.entity_id, packet.entity_id);
    assert!((decoded.movement.x - packet.movement.x).abs() < 0.000_1);
    assert!((decoded.movement.y - packet.movement.y).abs() < 0.000_1);
    assert!((decoded.movement.z - packet.movement.z).abs() < 0.000_1);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_change_game_mode_id_and_layout_match_javap() {
    assert_eq!(ServerboundChangeGameMode::ID, 0x05);
    let packet = ServerboundChangeGameMode {
        mode: GameMode::Creative,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x01]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundChangeGameMode::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_cooldown_id_and_layout_match_javap() {
    assert_eq!(ClientboundCooldown::ID, 0x16);
    let packet = ClientboundCooldown {
        cooldown_group: sample_identifier("minecraft:shield"),
        duration: 100,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, b"\x10minecraft:shield\x64");

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundCooldown::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_set_time_id_and_overworld_clock_layout_match_javap() {
    assert_eq!(ClientboundSetTime::ID, 0x73);
    let packet = ClientboundSetTime::overworld(12_345, 6_000, 1.0);
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        [
            12_345_i64.to_be_bytes().as_slice(),
            &[1, 0, 0xF0, 0x2E],
            0.0_f32.to_be_bytes().as_slice(),
            1.0_f32.to_be_bytes().as_slice(),
        ]
        .concat()
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSetTime::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_chat_command_id_and_layout_match_javap() {
    assert_eq!(ServerboundChatCommand::ID, 0x07);
    let packet = ServerboundChatCommand {
        command: "gamemode creative".to_string(),
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 17);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundChatCommand::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_chat_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundChat::ID, 0x09);
    let packet = ServerboundChat {
        message: "p34 hello".to_string(),
        timestamp_millis: 1234,
        salt: 55,
        signature: None,
        last_seen_offset: 2,
        last_seen_acknowledged: [0b0000_0011, 0, 0],
        last_seen_checksum: 0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 9);
    assert_eq!(&buf[1..10], b"p34 hello");
    assert_eq!(&buf[10..18], &1234_i64.to_be_bytes());
    assert_eq!(&buf[18..26], &55_i64.to_be_bytes());
    assert_eq!(buf[26], 0);
    assert_eq!(buf[27], 2);
    assert_eq!(&buf[28..31], &[0b0000_0011, 0, 0]);
    assert_eq!(buf[31], 0);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundChat::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_chat_ack_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundChatAck::ID, 0x06);
    let packet = ServerboundChatAck { offset: 300 };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0xAC, 0x02]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundChatAck::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn command_suggestion_packets_match_local_decompiled_sources() {
    assert_eq!(ServerboundCommandSuggestion::ID, 0x0F);
    assert_eq!(ClientboundCommandSuggestions::ID, 0x0F);

    let request = ServerboundCommandSuggestion {
        id: 42,
        command: "gamemode ".to_string(),
    };
    let mut buf = Vec::new();
    request.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            42, 0x09, b'g', b'a', b'm', b'e', b'm', b'o', b'd', b'e', b' ',
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundCommandSuggestion::decode(&mut cursor).unwrap(),
        request
    );
    assert!(cursor.is_empty());

    let response = ClientboundCommandSuggestions {
        id: request.id,
        start: request.command.len() as i32,
        length: 0,
        suggestions: Vec::new(),
    };
    let mut buf = Vec::new();
    response.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![42, 9, 0, 0]);
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundCommandSuggestions::decode(&mut cursor).unwrap(),
        response
    );
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_commands_literal_and_greedy_string_tree_matches_javap_layout() {
    assert_eq!(ClientboundCommands::ID, 0x10);
    let packet = ClientboundCommands {
        nodes: vec![
            CommandNode::root(vec![1, 3]),
            CommandNode::literal("gamemode", vec![2], false).restricted(true),
            CommandNode::literal("creative", Vec::new(), true).restricted(true),
            CommandNode::literal("give", vec![4], false).restricted(true),
            CommandNode::argument(
                "args",
                CommandArgumentParser::String(CommandStringKind::GreedyPhrase),
                Vec::new(),
                true,
            )
            .restricted(true),
        ],
        root_index: 0,
    };

    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 5); // node list length
    assert_eq!(buf[1], 0); // root flags
    assert_eq!(buf[2], 2); // root child count
    assert_eq!(buf[3], 1);
    assert_eq!(buf[4], 3);
    assert_eq!(buf[5], 0x21); // restricted literal, not executable

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundCommands::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

fn command_parser_packet(parser: CommandArgumentParser) -> ClientboundCommands {
    ClientboundCommands {
        nodes: vec![
            CommandNode::root(vec![1]),
            CommandNode::argument("v", parser, Vec::new(), true),
        ],
        root_index: 0,
    }
}

fn command_parser_wire_body(parser_wire: &[u8]) -> Vec<u8> {
    // Two nodes: root -> executable argument "v", followed by root index 0.
    let mut body = vec![2, 0, 1, 1, 6, 0, 1, b'v'];
    body.extend_from_slice(parser_wire);
    body.push(0);
    body
}

fn assert_command_parser_wire(parser: CommandArgumentParser, parser_wire: &[u8]) {
    let packet = command_parser_packet(parser);
    let expected = command_parser_wire_body(parser_wire);
    let mut encoded = Vec::new();
    packet.encode(&mut encoded).unwrap();
    assert_eq!(encoded, expected, "{packet:?}");
    let mut cursor = expected.as_slice();
    assert_eq!(ClientboundCommands::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn command_argument_parser_ids_and_properties_match_vanilla_26_3() {
    use CommandArgumentParser as P;
    // ArgumentTypeInfos.bootstrap registration order and SingletonArgumentInfo.
    let singleton_cases = [
        (P::Bool, 0),
        (P::GameProfile, 7),
        (P::BlockPos, 8),
        (P::ColumnPos, 9),
        (P::Vec3, 10),
        (P::Vec2, 11),
        (P::BlockState, 12),
        (P::BlockPredicate, 13),
        (P::ItemStack, 14),
        (P::ItemPredicate, 15),
        (P::Color, 16), // 26.3 renamed minecraft:color to minecraft:team_color
        (P::HexColor, 17),
        (P::Component, 18),
        (P::Style, 19),
        (P::Message, 20),
        (P::NbtCompoundTag, 21),
        (P::NbtTag, 22),
        (P::NbtPath, 23),
        (P::Objective, 24),
        (P::ObjectiveCriteria, 25),
        (P::Operation, 26),
        (P::Angle, 28),
        (P::Rotation, 29),
        (P::ScoreboardSlot, 30),
        (P::Swizzle, 32),
        (P::Team, 33),
        (P::ResourceLocation, 36),
        (P::EntityAnchor, 38),
        (P::GameMode, 42),
        (P::Uuid, 61),
    ];
    for (parser, id) in singleton_cases {
        assert_command_parser_wire(parser, &[id]);
    }
    for (kind, property) in [
        (CommandStringKind::SingleWord, 0),
        (CommandStringKind::QuotablePhrase, 1),
        (CommandStringKind::GreedyPhrase, 2),
    ] {
        assert_command_parser_wire(P::String(kind), &[5, property]);
    }
    for flags in 0..=3 {
        assert_command_parser_wire(
            P::Entity {
                single: flags & 1 != 0,
                players_only: flags & 2 != 0,
            },
            &[6, flags],
        );
    }
    for flags in 0..=1 {
        assert_command_parser_wire(
            P::ScoreHolder {
                multiple: flags != 0,
            },
            &[31, flags],
        );
    }
    assert_command_parser_wire(P::Time { min: 20 }, &[43, 0, 0, 0, 20]);
    assert_command_parser_wire(P::Time { min: -1 }, &[43, 255, 255, 255, 255]);
    for (parser, id) in [
        (
            P::Resource {
                registry: Identifier::parse("minecraft:entity_type").unwrap(),
            },
            46,
        ),
        (
            P::ResourceKey {
                registry: Identifier::parse("minecraft:enchantment").unwrap(),
            },
            47,
        ),
    ] {
        let name = if id == 46 {
            "minecraft:entity_type"
        } else {
            "minecraft:enchantment"
        };
        let mut wire = vec![id, name.len() as u8];
        wire.extend_from_slice(name.as_bytes());
        assert_command_parser_wire(parser, &wire);
    }
}

#[test]
fn command_numeric_parser_bounds_match_vanilla_flags_and_big_endian_properties() {
    use CommandArgumentParser as P;
    // Each numeric ArgumentInfo writes one flags byte, then min and max if set.
    // Cover no bound, min only, max only, and both bounds for every numeric type.
    for flags in 0..=3 {
        let has_min = flags & 1 != 0;
        let has_max = flags & 2 != 0;
        let mut integer_wire = vec![3, flags];
        let mut long_wire = vec![4, flags];
        let mut float_wire = vec![1, flags];
        let mut double_wire = vec![2, flags];
        if has_min {
            integer_wire.extend_from_slice(&(-257i32).to_be_bytes());
            long_wire.extend_from_slice(&(-4_294_967_297i64).to_be_bytes());
            float_wire.extend_from_slice(&(-1.25f32).to_bits().to_be_bytes());
            double_wire.extend_from_slice(&(-1.25f64).to_bits().to_be_bytes());
        }
        if has_max {
            integer_wire.extend_from_slice(&256i32.to_be_bytes());
            long_wire.extend_from_slice(&4_294_967_296i64.to_be_bytes());
            float_wire.extend_from_slice(&2.5f32.to_bits().to_be_bytes());
            double_wire.extend_from_slice(&2.5f64.to_bits().to_be_bytes());
        }
        assert_command_parser_wire(
            P::Integer {
                min: has_min.then_some(-257),
                max: has_max.then_some(256),
            },
            &integer_wire,
        );
        assert_command_parser_wire(
            P::Long {
                min: has_min.then_some(-4_294_967_297),
                max: has_max.then_some(4_294_967_296),
            },
            &long_wire,
        );
        assert_command_parser_wire(
            P::Float {
                min: has_min.then_some((-1.25f32).to_bits()),
                max: has_max.then_some(2.5f32.to_bits()),
            },
            &float_wire,
        );
        assert_command_parser_wire(
            P::Double {
                min: has_min.then_some((-1.25f64).to_bits()),
                max: has_max.then_some(2.5f64.to_bits()),
            },
            &double_wire,
        );
    }
    // Preserve the property bits even when the two bounds compare equal.
    assert_command_parser_wire(
        P::Float {
            min: Some((-0.0f32).to_bits()),
            max: Some(0.0f32.to_bits()),
        },
        &[1, 3, 128, 0, 0, 0, 0, 0, 0, 0],
    );
    assert_command_parser_wire(
        P::Double {
            min: Some((-0.0f64).to_bits()),
            max: Some(0.0f64.to_bits()),
        },
        &[2, 3, 128, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
    );
}

#[test]
fn command_argument_parser_rejects_invalid_bounds_flags_and_truncated_properties() {
    use CommandArgumentParser as P;
    let invalid_bounds = [
        P::Integer {
            min: Some(2),
            max: Some(1),
        },
        P::Long {
            min: Some(2),
            max: Some(1),
        },
        P::Float {
            min: Some(2.0f32.to_bits()),
            max: Some(1.0f32.to_bits()),
        },
        P::Double {
            min: Some(2.0f64.to_bits()),
            max: Some(1.0f64.to_bits()),
        },
        P::Float {
            min: Some(f32::NAN.to_bits()),
            max: None,
        },
        P::Float {
            min: None,
            max: Some(f32::INFINITY.to_bits()),
        },
        P::Double {
            min: Some(f64::NEG_INFINITY.to_bits()),
            max: None,
        },
        P::Double {
            min: None,
            max: Some(f64::NAN.to_bits()),
        },
    ];
    for parser in invalid_bounds {
        assert_eq!(
            command_parser_packet(parser).encode(&mut Vec::new()),
            Err(CodecError::NotSupported("invalid command numeric bounds")),
        );
    }
    // Invalid intervals and nonfinite floating bounds must also fail on decode.
    for (id, min, max) in [
        (3, 2i32.to_be_bytes().to_vec(), 1i32.to_be_bytes().to_vec()),
        (4, 2i64.to_be_bytes().to_vec(), 1i64.to_be_bytes().to_vec()),
        (
            1,
            2.0f32.to_bits().to_be_bytes().to_vec(),
            1.0f32.to_bits().to_be_bytes().to_vec(),
        ),
        (
            2,
            2.0f64.to_bits().to_be_bytes().to_vec(),
            1.0f64.to_bits().to_be_bytes().to_vec(),
        ),
        (
            1,
            f32::NAN.to_bits().to_be_bytes().to_vec(),
            1.0f32.to_bits().to_be_bytes().to_vec(),
        ),
        (
            2,
            0.0f64.to_bits().to_be_bytes().to_vec(),
            f64::INFINITY.to_bits().to_be_bytes().to_vec(),
        ),
    ] {
        let mut wire = vec![id, 3];
        wire.extend_from_slice(&min);
        wire.extend_from_slice(&max);
        let body = command_parser_wire_body(&wire);
        assert_eq!(
            ClientboundCommands::decode(&mut body.as_slice()),
            Err(CodecError::NotSupported("invalid command numeric bounds")),
        );
    }
    for id in [1, 2, 3, 4, 6, 31] {
        let body = command_parser_wire_body(&[id, 128]);
        assert_eq!(
            ClientboundCommands::decode(&mut body.as_slice()),
            Err(CodecError::NotSupported("invalid command argument flags")),
        );
    }
    for wire in [&[5, 3][..], &[57][..]] {
        let body = command_parser_wire_body(wire);
        assert!(matches!(
            ClientboundCommands::decode(&mut body.as_slice()),
            Err(CodecError::NotSupported(_)),
        ));
    }
    for (id, property) in [
        (1, vec![3, 0, 0, 0, 0, 0, 0, 0, 0]),
        (2, vec![3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        (3, vec![3, 0, 0, 0, 0, 0, 0, 0, 0]),
        (4, vec![3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]),
        (6, vec![0]),
        (31, vec![0]),
        (43, vec![0, 0, 0, 0]),
        (46, vec![1, b'a']),
        (47, vec![1, b'a']),
    ] {
        for available in 0..property.len() {
            let mut body = vec![2, 0, 1, 1, 6, 0, 1, b'v', id];
            body.extend_from_slice(&property[..available]);
            assert!(
                matches!(
                    ClientboundCommands::decode(&mut body.as_slice()),
                    Err(CodecError::Underflow { .. }),
                ),
                "parser id {id}, property bytes {available}"
            );
        }
    }
}

#[test]
fn command_registry_properties_precede_custom_suggestions() {
    let registry = Identifier::parse("minecraft:entity_type").unwrap();
    let suggestions = Identifier::parse("minecraft:ask_server").unwrap();
    let mut packet = command_parser_packet(CommandArgumentParser::Resource { registry });
    if let CommandNodeKind::Argument {
        suggestions: target,
        ..
    } = &mut packet.nodes[1].kind
    {
        *target = Some(suggestions);
    }
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();
    let mut expected = vec![2, 0, 1, 1, 0x16, 0, 1, b'v', 46, 21];
    expected.extend_from_slice(b"minecraft:entity_type");
    expected.push(20);
    expected.extend_from_slice(b"minecraft:ask_server");
    expected.push(0);
    assert_eq!(body, expected);
    let mut cursor = body.as_slice();
    assert_eq!(ClientboundCommands::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn clientbound_system_chat_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ClientboundSystemChat::ID, 0x7C);
    let packet = ClientboundSystemChat {
        content_nbt: vec![0x0A, 0x00],
        overlay: false,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x0A, 0x00, 0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSystemChat::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn client_tick_chunk_batch_and_player_loaded_packets_match_local_decompiled_sources() {
    assert_eq!(ServerboundChunkBatchReceived::ID, 0x0B);
    assert_eq!(ServerboundClientTickEnd::ID, 0x0D);
    assert_eq!(ServerboundPlayerLoaded::ID, 0x2C);

    let batch = ServerboundChunkBatchReceived {
        desired_chunks_per_tick: 12.5,
    };
    let mut buf = Vec::new();
    batch.encode(&mut buf).unwrap();
    assert_eq!(buf, 12.5_f32.to_be_bytes());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundChunkBatchReceived::decode(&mut cursor).unwrap(),
        batch
    );
    assert!(cursor.is_empty());

    let mut buf = Vec::new();
    ServerboundClientTickEnd.encode(&mut buf).unwrap();
    assert!(buf.is_empty());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundClientTickEnd::decode(&mut cursor).unwrap(),
        ServerboundClientTickEnd
    );
    assert!(cursor.is_empty());

    let mut buf = Vec::new();
    ServerboundPlayerLoaded.encode(&mut buf).unwrap();
    assert!(buf.is_empty());
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundPlayerLoaded::decode(&mut cursor).unwrap(),
        ServerboundPlayerLoaded
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_resource_pack_id_and_layout_match_local_decompiled_sources() {
    assert_eq!(ServerboundResourcePack::ID, 0x32);
    let packet = ServerboundResourcePack {
        status: ResourcePackStatus {
            id: Uuid::from_u128(0x0011_2233_4455_6677_8899_aabb_ccdd_eeff),
            action: ResourcePackAction::FailedReload,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0x00, 0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88, 0x99, 0xaa, 0xbb, 0xcc, 0xdd,
            0xee, 0xff, 0x06,
        ]
    );
    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ServerboundResourcePack::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn serverbound_player_command_id_and_layout_match_protocol_dump() {
    assert_eq!(ServerboundPlayerCommand::ID, 0x2A);
    // Local 26.1.2 ServerboundPlayerCommandPacket$Action.values() has seven
    // actions; shift keys moved to ServerboundPlayerInputPacket.
    for (ordinal, action) in [
        (0, PlayerCommandAction::StopSleeping),
        (1, PlayerCommandAction::StartSprinting),
        (2, PlayerCommandAction::StopSprinting),
        (3, PlayerCommandAction::StartRidingJump),
        (4, PlayerCommandAction::StopRidingJump),
        (5, PlayerCommandAction::OpenInventory),
        (6, PlayerCommandAction::StartFallFlying),
    ] {
        let packet = ServerboundPlayerCommand {
            entity_id: 123,
            action,
            data: 0,
        };
        let bytes = vec![123, ordinal, 0];
        let mut encoded = Vec::new();
        packet.encode(&mut encoded).unwrap();
        assert_eq!(encoded, bytes);
        let mut cursor = bytes.as_slice();
        assert_eq!(
            ServerboundPlayerCommand::decode(&mut cursor).unwrap(),
            packet
        );
        assert!(cursor.is_empty());
    }
    for ordinal in [7, 8, 127] {
        assert!(ServerboundPlayerCommand::decode(&mut &[123, ordinal, 0][..]).is_err());
    }
}

#[test]
fn serverbound_player_input_id_and_layout_match_javap() {
    assert_eq!(ServerboundPlayerInput::ID, 0x2B);
    let packet = ServerboundPlayerInput {
        input: PlayerInput {
            forward: true,
            backward: false,
            left: false,
            right: true,
            jump: true,
            shift: false,
            sprint: true,
        },
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x59]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(ServerboundPlayerInput::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn player_abilities_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundPlayerAbilities::ID, 0x41);
    let packet = ClientboundPlayerAbilities {
        invulnerable: true,
        flying: false,
        can_fly: true,
        instabuild: true,
        flying_speed: 0.05,
        walking_speed: 0.1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf[0], 0x0d);
    assert_eq!(&buf[1..5], &0.05_f32.to_be_bytes());
    assert_eq!(&buf[5..9], &0.1_f32.to_be_bytes());

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundPlayerAbilities::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn change_difficulty_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundChangeDifficulty::ID, 0x0A);
    let packet = ClientboundChangeDifficulty {
        difficulty: 1,
        locked: false,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(buf, vec![0x01, 0x00]);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundChangeDifficulty::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn update_recipes_stonecutter_id_and_layout_match_local_vanilla_sources() {
    assert_eq!(ClientboundUpdateRecipes::ID, 0x88);
    let packet = ClientboundUpdateRecipes {
        item_sets: vec![RecipePropertySet {
            key: Identifier::parse("x:y").unwrap(),
            item_ids: vec![12, 13],
        }],
        stonecutter_recipes: vec![StonecutterRecipeEntry {
            input: RecipeBookIngredient::Items(vec![14, 15]),
            result: RecipeBookSlotDisplay::ItemStack(Box::new(ItemStack::new(16, 2))),
        }],
    };

    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            1, // one RecipePropertySet map entry
            3, b'x', b':', b'y', // resource key identifier
            2, 12, 13, // RecipePropertySet's ordinary item list
            1,  // one stonecutter recipe
            3, 14, 15, // direct Ingredient holder set: size + 1, item ids
            5, 16, 2, 0, 0, // item-stack SlotDisplay with empty component patch
        ]
    );

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundUpdateRecipes::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn set_health_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundSetHealth::ID, 0x6A);
    let packet = ClientboundSetHealth {
        health: 20.0,
        food: 20,
        saturation: 5.0,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[0..4], &20.0_f32.to_be_bytes());
    assert_eq!(buf[4], 20);
    assert_eq!(&buf[5..9], &5.0_f32.to_be_bytes());

    let mut cursor: &[u8] = &buf;
    assert_eq!(ClientboundSetHealth::decode(&mut cursor).unwrap(), packet);
    assert!(cursor.is_empty());
}

#[test]
fn set_experience_id_and_wire_layout_match_javap() {
    assert_eq!(ClientboundSetExperience::ID, 0x69);
    let packet = ClientboundSetExperience {
        experience_progress: 0.5,
        total_experience: 9,
        experience_level: 1,
    };
    let mut buf = Vec::new();
    packet.encode(&mut buf).unwrap();
    assert_eq!(&buf[0..4], &0.5_f32.to_be_bytes());
    assert_eq!(buf[4], 1);
    assert_eq!(buf[5], 9);

    let mut cursor: &[u8] = &buf;
    assert_eq!(
        ClientboundSetExperience::decode(&mut cursor).unwrap(),
        packet
    );
    assert!(cursor.is_empty());
}

#[test]
fn remove_player_packets_round_trip() {
    let uuid = Uuid::from_u128(0x00112233445566778899aabbccddeeff);
    round_trip(RemoveEntities {
        entity_ids: vec![42, 43],
    });
    // Batched despawns of dense mob areas exceed 1024 ids in one packet.
    round_trip(RemoveEntities {
        entity_ids: (0..5000).collect(),
    });
    round_trip(PlayerInfoRemove {
        profile_ids: vec![uuid],
    });
    let mut buf = Vec::new();
    RotateHead {
        entity_id: 42,
        head_yaw: 90.0,
    }
    .encode(&mut buf)
    .unwrap();
    assert_eq!(buf, vec![42, 64]);
}

// ---- SetCenterChunk ----

#[test]
fn set_center_chunk_id_matches_javap() {
    assert_eq!(SetCenterChunk::ID, 0x60);
}

#[test]
fn set_center_chunk_round_trips() {
    for (x, z) in [(0, 0), (1, -1), (-100_000, 100_000), (i32::MIN, i32::MAX)] {
        round_trip(SetCenterChunk {
            chunk_x: x,
            chunk_z: z,
        });
    }
}

// ---- LevelChunkWithLight ----

fn minimal_chunk_packet() -> LevelChunkWithLight {
    LevelChunkWithLight {
        chunk_x: 0,
        chunk_z: 0,
        heightmaps: Vec::new(),
        data: Vec::new(),
        block_entities: Vec::new(),
        light: LightData::empty(),
    }
}

#[test]
fn level_chunk_with_light_id_matches_javap() {
    // 0x2D = game-CB index 45 (CLIENTBOUND_LEVEL_CHUNK_WITH_LIGHT).
    assert_eq!(LevelChunkWithLight::ID, 0x2E);
}

#[test]
fn level_chunk_with_light_empty_byte_layout() {
    // The all-empty form: i32 x, i32 z, six VarInt(0)s for
    // (heightmap-count, data-len, block-entity-count, four BitSets),
    // two VarInt(0)s for the sky/block update lists.
    // Total: 4 + 4 + 9*1 = 17 bytes.
    let mut buf = Vec::new();
    minimal_chunk_packet().encode(&mut buf).unwrap();
    assert_eq!(
        buf,
        vec![
            0, 0, 0, 0, // chunk_x = 0
            0, 0, 0, 0, // chunk_z = 0
            0, // heightmap count = 0
            0, // chunk data length = 0
            0, // block entity count = 0
            0, // sky_y_mask bytes = 0
            0, // block_y_mask bytes = 0
            0, // empty_sky_y_mask bytes = 0
            0, // empty_block_y_mask bytes = 0
            0, // sky_updates count = 0
            0, // block_updates count = 0
        ]
    );
}

#[test]
fn level_chunk_with_light_round_trips_empty() {
    round_trip(minimal_chunk_packet());
}

#[test]
fn level_chunk_with_light_rejects_infeasible_block_entity_count_before_decode() {
    let mut body = Vec::new();
    body.write_i32(0);
    body.write_i32(0);
    body.write_varint(0);
    body.write_varint(0);
    body.write_varint(MAX_CHUNK_BLOCK_ENTITIES as i32);

    assert_eq!(
        LevelChunkWithLight::decode(&mut body.as_slice()).unwrap_err(),
        CodecError::Underflow {
            needed: MAX_CHUNK_BLOCK_ENTITIES * MIN_BLOCK_ENTITY_BYTES,
            available: 0,
        }
    );
}

#[test]
fn level_chunk_with_light_round_trips_with_heightmaps_and_data() {
    round_trip(LevelChunkWithLight {
        chunk_x: -3,
        chunk_z: 7,
        heightmaps: vec![
            ChunkHeightmap {
                type_id: ChunkHeightmap::MOTION_BLOCKING,
                data: vec![0x0123_4567_89AB_CDEF, 0],
            },
            ChunkHeightmap {
                type_id: ChunkHeightmap::WORLD_SURFACE,
                data: vec![-1; 4],
            },
        ],
        data: (0..512).map(|i| (i & 0xFF) as u8).collect(),
        block_entities: Vec::new(),
        light: LightData::empty(),
    });
}

#[test]
fn level_chunk_with_light_round_trips_with_non_empty_light_masks() {
    round_trip(LevelChunkWithLight {
        chunk_x: 0,
        chunk_z: 0,
        heightmaps: Vec::new(),
        data: Vec::new(),
        block_entities: Vec::new(),
        light: LightData {
            // All 26 indexable Y sections marked "has data".
            sky_y_mask: vec![(1i64 << 26) - 1],
            block_y_mask: vec![(1i64 << 26) - 1],
            empty_sky_y_mask: Vec::new(),
            empty_block_y_mask: Vec::new(),
            sky_updates: vec![vec![0xFFu8; LightData::LIGHT_LAYER_BYTES]; 26],
            block_updates: vec![vec![0u8; LightData::LIGHT_LAYER_BYTES]; 26],
        },
    });
}

#[test]
fn light_masks_match_local_vanilla_263_byte_bitsets() {
    // ByteBufCodecs$15 uses BitSet.toByteArray/valueOf(byte[]): a VarInt
    // byte count, little-endian bits, and no trailing zero bytes.
    let light = LightData {
        sky_y_mask: vec![1 << 9],
        block_y_mask: vec![i64::MIN],
        empty_sky_y_mask: vec![0, 1],
        empty_block_y_mask: vec![0],
        sky_updates: vec![vec![0xFF; LightData::LIGHT_LAYER_BYTES]],
        block_updates: vec![vec![0x12; LightData::LIGHT_LAYER_BYTES]],
    };
    let update = LightUpdate {
        chunk_x: 0,
        chunk_z: 0,
        light: light.clone(),
    };
    let mut bytes = Vec::new();
    update.encode(&mut bytes).unwrap();
    let mut vanilla: &[u8] = &bytes;
    assert_eq!(vanilla.read_varint().unwrap(), 0);
    assert_eq!(vanilla.read_varint().unwrap(), 0);
    for expected in [
        &[0, 2][..],
        &[0, 0, 0, 0, 0, 0, 0, 0x80][..],
        &[0, 0, 0, 0, 0, 0, 0, 0, 1][..],
        &[][..],
    ] {
        assert_eq!(vanilla.read_byte_array(32768).unwrap(), expected);
    }
    for expected in [&light.sky_updates[0], &light.block_updates[0]] {
        assert_eq!(vanilla.read_varint().unwrap(), 1);
        assert_eq!(vanilla.read_byte_array(2048).unwrap(), *expected);
    }
    assert!(vanilla.is_empty());
    let decoded = LightUpdate::decode(&mut bytes.as_slice()).unwrap();
    assert!(decoded.light.empty_block_y_mask.is_empty());
    assert_eq!(decoded.light.sky_y_mask, light.sky_y_mask);
    assert_eq!(decoded.light.block_y_mask, light.block_y_mask);
    assert_eq!(decoded.light.empty_sky_y_mask, light.empty_sky_y_mask);

    let mut chunk = minimal_chunk_packet();
    chunk.light = light;
    let mut chunk_bytes = Vec::new();
    chunk.encode(&mut chunk_bytes).unwrap();
    // Empty chunk envelope: coordinates, heightmaps, data and sidecar count.
    assert_eq!(&chunk_bytes[11..], &bytes[2..]);
}

#[test]
fn level_chunk_with_light_round_trips_with_block_entities() {
    // Network-NBT compound with one byte tag inside, modelling a
    // (toy) block entity payload.
    let nbt = Tag::Compound(vec![("k".to_string(), Tag::Byte(42))]);
    round_trip(LevelChunkWithLight {
        chunk_x: 4,
        chunk_z: -8,
        heightmaps: Vec::new(),
        data: Vec::new(),
        block_entities: vec![BlockEntityInfo {
            packed_xz: (3 << 4) | 9,
            y: 64,
            type_id: 7,
            nbt,
        }],
        light: LightData::empty(),
    });
}

#[test]
fn level_chunk_with_light_rejects_oversized_chunk_data_on_decode() {
    // Hand-encode an i32(0), i32(0), heightmap-count VarInt(0), then
    // a VarInt declaring (MAX_CHUNK_DATA_LEN + 1) bytes of chunk
    // data. Decode should reject before allocating.
    let mut buf = Vec::new();
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.extend_from_slice(&0i32.to_be_bytes());
    buf.write_varint(0);
    buf.write_varint((MAX_CHUNK_DATA_LEN + 1) as i32);
    let mut cursor: &[u8] = &buf;
    let err = LevelChunkWithLight::decode(&mut cursor).unwrap_err();
    assert!(matches!(err, CodecError::ArrayTooLong { .. }));
}

// ---- M5.a: serverbound interaction packets ----

include!("tests/merchant.rs");
include!("tests/serverbound_and_slots.rs");
include!("tests/serverbound_encoder_atomicity.rs");
include!("tests/set_passengers.rs");

#[test]
fn transfer_packet_round_trip_and_wire_id() {
    assert_eq!(ClientboundTransfer::ID, 0x84);
    round_trip(ClientboundTransfer {
        host: "lobby.example.com".into(),
        port: 25565,
    });
}
