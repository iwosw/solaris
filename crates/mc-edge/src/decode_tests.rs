//! decode.rs unit tests: every mapped packet -> `PlayerInput`, plus
//! invalid-chat and unknown-id rejection.

use super::*;
use mc_core::boundary::{ClickMode, ConnectionId, DigAction, Hand, PlayerInput};
use mc_protocol::codec::WriteMc;
use mc_protocol::packets::play::{
    ClientCommandAction, ContainerInput, Direction, InteractionHand, MovePlayerFlags,
    PlayerActionKind, PlayerCommandAction, ServerboundAttack, ServerboundChat, ServerboundChatAck,
    ServerboundChatCommand, ServerboundChunkBatchReceived, ServerboundClientCommand,
    ServerboundClientInformation, ServerboundClientTickEnd, ServerboundCommandSuggestion,
    ServerboundContainerClick, ServerboundContainerClose, ServerboundCustomPayload,
    ServerboundInteract, ServerboundKeepAlive, ServerboundMovePlayerPos,
    ServerboundMovePlayerPosRot, ServerboundMovePlayerRot, ServerboundMovePlayerStatusOnly,
    ServerboundPlaceRecipe, ServerboundPlayerAction, ServerboundPlayerCommand,
    ServerboundPlayerInput, ServerboundResourcePack, ServerboundSetCarriedItem,
    ServerboundSignUpdate, ServerboundUseItem, ServerboundUseItemOn, pack_block_pos,
};
use mc_protocol::packets::{
    ChatVisibility, ClientInformation, CustomPayload, MainHand, Packet, ParticleStatus,
    ResourcePackAction, ResourcePackStatus,
};

fn conn() -> ConnectionId {
    ConnectionId(7)
}

fn state() -> DecodeState {
    DecodeState::default()
}

fn body<P: Packet>(packet: &P) -> Vec<u8> {
    let mut buf = Vec::new();
    packet.encode(&mut buf).expect("test packet encodes");
    buf
}

fn decode(id: i32, body: Vec<u8>, state: &mut DecodeState) -> Result<Vec<PlayerInput>, String> {
    decode_play_frame_for(conn(), id, &body, state)
}

#[test]
fn edit_book_wire_reaches_native_input_and_rejects_trailing_bytes() {
    // Local 26.1.2 packet report + ServerboundEditBookPacket javap:
    // slot VarInt, bounded page strings, optional title string.
    let bytes = vec![0, 1, 4, b'p', b'a', b'g', b'e', 1, 1, b'T'];
    assert_eq!(
        decode(24, bytes.clone(), &mut state()).unwrap(),
        vec![PlayerInput::EditBook {
            connection: conn(),
            slot: 0,
            pages: vec!["page".into()],
            title: Some("T".into()),
        }]
    );
    let mut trailing = bytes;
    trailing.push(0);
    assert!(decode(24, trailing, &mut state()).is_err());
}

#[test]
fn elytra_start_command_reaches_authoritative_core() {
    // Local 26.1.2 GameProtocols + ServerboundPlayerCommandPacket$Action:
    // packet 0x2A, entity 1, START_FALL_FLYING ordinal 6, data 0.
    // Do not encode with the enum under test: that hid its stale ordinal 8.
    let inputs = decode(0x2A, vec![1, 6, 0], &mut state()).unwrap();
    assert_eq!(
        inputs,
        vec![PlayerInput::StartFallFlying { connection: conn() }]
    );
}

#[test]
fn teleport_confirmation_reaches_the_core_and_rejects_malformed_bodies() {
    use mc_protocol::packets::play::ConfirmTeleportation;
    let packet = ConfirmTeleportation { teleport_id: 37 };
    assert_eq!(
        decode(ConfirmTeleportation::ID, body(&packet), &mut state()).unwrap(),
        vec![PlayerInput::ConfirmTeleport {
            connection: conn(),
            teleport_id: 37,
        }]
    );
    assert!(decode(ConfirmTeleportation::ID, Vec::new(), &mut state()).is_err());
    let mut trailing = body(&packet);
    trailing.push(0);
    assert!(decode(ConfirmTeleportation::ID, trailing, &mut state()).is_err());
}

#[test]
fn crafter_slot_toggle_exact_wire_reaches_core_and_rejects_trailing_bytes() {
    assert_eq!(
        decode(0x14, vec![8, 7, 0], &mut state()).unwrap(),
        vec![PlayerInput::ContainerSlotState {
            connection: conn(),
            window: 7,
            slot: 8,
            enabled: false
        }]
    );
    assert_eq!(
        decode(0x14, vec![0, 7, 1], &mut state()).unwrap(),
        vec![PlayerInput::ContainerSlotState {
            connection: conn(),
            window: 7,
            slot: 0,
            enabled: true
        }]
    );
    assert!(decode(0x14, vec![8, 7, 1, 0], &mut state()).is_err());
    assert!(decode(0x14, vec![8, 0x80, 0x02, 1], &mut state()).is_err());
}

#[test]
fn merchant_select_trade_matches_vanilla_varint_and_rejects_trailing_bytes() {
    // Local 26.1.2 javap: GameProtocols registration 51; SelectTrade.write
    // writes precisely one VarInt, with no container or revision field.
    use mc_protocol::packets::play::ServerboundSelectTrade;
    assert_eq!(ServerboundSelectTrade::ID, 0x33);
    assert_eq!(
        decode(0x33, vec![0xac, 0x02], &mut state()).unwrap(),
        vec![PlayerInput::SelectTrade {
            connection: conn(),
            offer: 300
        }]
    );
    assert!(decode(0x33, vec![0x01, 0x00], &mut state()).is_err());
    assert!(decode(0x33, vec![0xff, 0xff, 0xff, 0xff, 0x0f], &mut state()).is_err());
}

#[test]
fn beacon_set_effect_reaches_core_with_zero_based_ids() {
    // Local 26.1.2 javap: SERVERBOUND_SET_BEACON is the 53rd serverbound
    // game addPacket call = wire id 0x34; body is two `optional(holderRegistry)`
    // fields: one boolean each, then a VarInt zero-based mob_effect id.
    use mc_protocol::packets::play::ServerboundSetBeacon;
    assert_eq!(ServerboundSetBeacon::ID, 0x34);
    assert_eq!(
        decode(0x34, vec![1, 1, 1, 9], &mut state()).unwrap(),
        vec![PlayerInput::SetBeacon {
            connection: conn(),
            primary: Some(1),
            secondary: Some(9),
        }]
    );
    assert_eq!(
        decode(0x34, vec![0, 0], &mut state()).unwrap(),
        vec![PlayerInput::SetBeacon {
            connection: conn(),
            primary: None,
            secondary: None,
        }]
    );
    assert!(decode(0x34, vec![1, 1], &mut state()).is_err());
    assert!(decode(0x34, vec![1, 1, 1, 9, 0], &mut state()).is_err());
}

#[test]
fn workstation_offer_and_anvil_name_reach_core() {
    use mc_protocol::packets::play::{ServerboundContainerButtonClick, ServerboundRenameItem};
    let mut s = state();
    assert_eq!(
        decode(
            ServerboundContainerButtonClick::ID,
            body(&ServerboundContainerButtonClick {
                container_id: 7,
                button_id: 2,
            }),
            &mut s
        )
        .unwrap(),
        vec![PlayerInput::ContainerButton {
            connection: conn(),
            window: 7,
            button: 2
        }]
    );
    assert_eq!(
        decode(
            ServerboundRenameItem::ID,
            body(&ServerboundRenameItem {
                name: "Blade".into()
            }),
            &mut s
        )
        .unwrap(),
        vec![PlayerInput::RenameItem {
            connection: conn(),
            name: "Blade".into()
        }]
    );
    assert!(
        decode(
            ServerboundContainerButtonClick::ID,
            body(&ServerboundContainerButtonClick {
                container_id: 256,
                button_id: 0,
            }),
            &mut s
        )
        .is_err()
    );
}

#[test]
fn move_pos_merges_with_last_rotation() {
    let mut s = state();
    s.last_yaw = 1.0;
    s.last_pitch = 2.0;
    let out = decode(
        ServerboundMovePlayerPos::ID,
        body(&ServerboundMovePlayerPos {
            x: 10.0,
            y: 65.0,
            z: -3.0,
            flags: MovePlayerFlags::new(true, false),
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::Move {
            connection: conn(),
            position: [10.0, 65.0, -3.0],
            yaw: 1.0,
            pitch: 2.0,
            on_ground: true,
        }]
    );
}

#[test]
fn move_pos_rot_sets_all_axes() {
    let mut s = state();
    let out = decode(
        ServerboundMovePlayerPosRot::ID,
        body(&ServerboundMovePlayerPosRot {
            x: 1.0,
            y: 2.0,
            z: 3.0,
            yaw: 90.0,
            pitch: -45.0,
            flags: MovePlayerFlags::new(false, false),
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::Move {
            connection: conn(),
            position: [1.0, 2.0, 3.0],
            yaw: 90.0,
            pitch: -45.0,
            on_ground: false,
        }]
    );
    // A later Pos keeps the rotation.
    let out = decode(
        ServerboundMovePlayerPos::ID,
        body(&ServerboundMovePlayerPos {
            x: 4.0,
            y: 5.0,
            z: 6.0,
            flags: MovePlayerFlags::new(true, false),
        }),
        &mut s,
    )
    .unwrap();
    assert!(
        matches!(out[0], PlayerInput::Move { yaw, pitch, .. } if yaw == 90.0 && pitch == -45.0)
    );
}

#[test]
fn move_rot_and_status_only_preserve_absent_coordinates() {
    let mut s = state();
    let out = decode(
        ServerboundMovePlayerRot::ID,
        body(&ServerboundMovePlayerRot {
            yaw: 180.0,
            pitch: 10.0,
            flags: MovePlayerFlags::new(true, false),
        }),
        &mut s,
    )
    .unwrap();
    assert!(matches!(out[0], PlayerInput::MoveRotation { yaw, .. } if yaw == 180.0));
    let out = decode(
        ServerboundMovePlayerStatusOnly::ID,
        body(&ServerboundMovePlayerStatusOnly {
            flags: MovePlayerFlags::new(false, false),
        }),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out[0],
        PlayerInput::MoveStatus {
            on_ground: false,
            ..
        }
    ));
}

#[test]
fn player_action_maps_dig_drop_release_swap() {
    let mut s = state();
    let mk = |action| {
        body(&ServerboundPlayerAction {
            action,
            position: mc_protocol::packets::play::pack_block_pos(1, 2, 3),
            direction: Direction::Up,
            sequence: 9,
        })
    };
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::StartDestroyBlock),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out[0],
        PlayerInput::Dig {
            action: DigAction::Start,
            sequence: 9,
            ..
        }
    ));
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::AbortDestroyBlock),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out[0],
        PlayerInput::Dig {
            action: DigAction::Cancel,
            ..
        }
    ));
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::StopDestroyBlock),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out[0],
        PlayerInput::Dig {
            action: DigAction::Finish,
            ..
        }
    ));
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::DropAllItems),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::DropSelected {
            connection: conn(),
            whole_stack: true,
        }]
    );
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::DropItem),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::DropSelected {
            connection: conn(),
            whole_stack: false,
        }]
    );
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::ReleaseUseItem),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::ReleaseUseItem { connection: conn() }]
    );
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::SwapItemWithOffhand),
        &mut s,
    )
    .unwrap();
    assert_eq!(out, vec![PlayerInput::SwapHands { connection: conn() }]);
    // Stab has no mapping.
    let out = decode(
        ServerboundPlayerAction::ID,
        mk(PlayerActionKind::Stab),
        &mut s,
    )
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn use_item_on_decodes() {
    let mut s = state();
    let out = decode(
        ServerboundUseItemOn::ID,
        body(&ServerboundUseItemOn {
            hand: InteractionHand::MainHand,
            position: mc_protocol::packets::play::pack_block_pos(5, 64, -7),
            direction: Direction::North,
            cursor_x: 0.5,
            cursor_y: 0.25,
            cursor_z: 0.75,
            inside: false,
            world_border_hit: false,
            sequence: 3,
        }),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out[0],
        PlayerInput::UseItemOn {
            hand: Hand::Main,
            face: 2,
            sequence: 3,
            ..
        }
    ));
}

#[test]
fn use_item_decodes() {
    let mut s = state();
    let out = decode(
        ServerboundUseItem::ID,
        body(&ServerboundUseItem {
            hand: InteractionHand::OffHand,
            sequence: 4,
            y_rot: 12.0,
            x_rot: -3.0,
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::UseItem {
            connection: conn(),
            hand: Hand::Off,
            yaw: 12.0,
            pitch: -3.0,
            sequence: 4,
        }]
    );
}

#[test]
fn swing_emits_nothing() {
    let mut s = state();
    let out = decode(
        0x3F,
        body(&mc_protocol::packets::play::ServerboundSwing {
            hand: InteractionHand::MainHand,
        }),
        &mut s,
    )
    .unwrap();
    assert!(out.is_empty());
}

#[test]
fn swing_before_or_after_attack_never_adds_a_second_attack_intent() {
    for swing_first in [true, false] {
        let mut s = state();
        let attack = (
            ServerboundAttack::ID,
            body(&ServerboundAttack { entity_id: 159 }),
        );
        let swing = (
            0x3F,
            body(&mc_protocol::packets::play::ServerboundSwing {
                hand: InteractionHand::MainHand,
            }),
        );
        let frames = if swing_first {
            [swing, attack]
        } else {
            [attack, swing]
        };
        let mut inputs = Vec::new();
        for (id, frame) in frames {
            inputs.extend(decode(id, frame, &mut s).unwrap());
        }
        assert_eq!(
            inputs,
            vec![PlayerInput::Attack {
                connection: conn(),
                target: mc_core::boundary::EntityId(159),
            }],
            "animation has no independent intent that could reset combat strength"
        );
    }
}

#[test]
fn attack_and_interact_decode() {
    let mut s = state();
    let out = decode(
        ServerboundAttack::ID,
        body(&ServerboundAttack { entity_id: 42 }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::Attack {
            connection: conn(),
            target: mc_core::boundary::EntityId(42),
        }]
    );
    let bytes = body(&ServerboundInteract {
        entity_id: 43,
        hand: InteractionHand::OffHand,
        location: mc_protocol::packets::play::EntityVec3 {
            x: 0.0,
            y: 1.75,
            z: 0.0,
        },
        using_secondary_action: false,
    });
    let location = ServerboundInteract::decode(&mut bytes.as_ref())
        .unwrap()
        .location;
    assert!((location.y - 1.75).abs() < 0.0001, "LP vector quantization");
    let out = decode(ServerboundInteract::ID, bytes, &mut s).unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::InteractEntity {
            connection: conn(),
            target: mc_core::boundary::EntityId(43),
            hand: Hand::Off,
            location: [location.x, location.y, location.z],
        }]
    );
}

#[test]
fn select_slot_validates_range() {
    let mut s = state();
    let out = decode(
        ServerboundSetCarriedItem::ID,
        body(&ServerboundSetCarriedItem { slot: 5 }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::SelectSlot {
            connection: conn(),
            slot: 5
        }]
    );
    assert!(
        decode(
            ServerboundSetCarriedItem::ID,
            body(&ServerboundSetCarriedItem { slot: 9 }),
            &mut s
        )
        .is_err()
    );
}

#[test]
fn chat_validates_length_and_controls() {
    let mut s = state();
    let mk = |message: String| {
        let mut buf = Vec::new();
        buf.write_string(&message, 256).expect("test chat encodes");
        buf.write_i64(0);
        buf.write_i64(0);
        buf.write_bool(false);
        buf.write_varint(0);
        buf.extend_from_slice(&[0, 0, 0]);
        buf.write_i8(0);
        buf
    };
    let out = decode(ServerboundChat::ID, mk("hello".to_owned()), &mut s).unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::Chat {
            connection: conn(),
            message: "hello".to_owned(),
        }]
    );
    assert!(decode(ServerboundChat::ID, mk("a\nb".to_owned()), &mut s).is_err());
    // Overlong: hand-build the frame (write_string rejects at encode time,
    // but the wire can still carry it; decode must reject).
    let mut raw = vec![0xFF, 0x02];
    raw.extend_from_slice(&vec![b'x'; 300]);
    raw.extend_from_slice(&[
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ]);
    assert!(decode(ServerboundChat::ID, raw, &mut s).is_err());
}

#[test]
fn chat_command_strips_slash_verbatim() {
    let mut s = state();
    for (raw, want) in [
        ("kill", "kill"),
        ("/kill", "kill"),
        (
            "/debug give minecraft:apple 2 1",
            "debug give minecraft:apple 2 1",
        ),
    ] {
        let out = decode(
            ServerboundChatCommand::ID,
            body(&ServerboundChatCommand {
                command: raw.to_owned(),
            }),
            &mut s,
        )
        .unwrap();
        assert_eq!(
            out,
            vec![PlayerInput::Command {
                connection: conn(),
                command: want.to_owned(),
                permission_level: 0,
            }],
            "raw={raw}"
        );
    }
    // Slash chat via 0x09 stays Chat (core-side routing owns it).
    let mut buf = Vec::new();
    buf.write_string("/kill", 256).unwrap();
    buf.write_i64(0);
    buf.write_i64(0);
    buf.write_bool(false);
    buf.write_varint(0);
    buf.extend_from_slice(&[0, 0, 0]);
    buf.write_i8(0);
    let out = decode(ServerboundChat::ID, buf, &mut s).unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::Chat {
            connection: conn(),
            message: "/kill".to_owned(),
        }]
    );
}

#[test]
fn container_click_close_place_recipe_decode() {
    let mut s = state();
    let out = decode(
        ServerboundContainerClick::ID,
        body(&ServerboundContainerClick {
            container_id: 1,
            state_id: 2,
            slot_num: 10,
            button_num: 0,
            container_input: ContainerInput::QuickMove,
            changed_slots: Vec::new(),
            carried_item: mc_protocol::packets::play::HashedStack::Empty,
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::ContainerClick {
            connection: conn(),
            window: 1,
            state_id: 2,
            slot: 10,
            button: 0,
            mode: ClickMode::QuickMove,
        }]
    );
    let out = decode(
        ServerboundContainerClose::ID,
        body(&ServerboundContainerClose { container_id: 1 }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::ContainerClose {
            connection: conn(),
            window: 1
        }]
    );
    let out = decode(
        ServerboundPlaceRecipe::ID,
        body(&ServerboundPlaceRecipe {
            container_id: 2,
            recipe_display_id: 7,
            use_max_items: true,
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(
        out,
        vec![PlayerInput::PlaceRecipe {
            connection: conn(),
            window: 2,
            recipe: 7,
            make_all: true,
        }]
    );
}

#[test]
fn client_commands_decode_respawn_and_statistics() {
    let mut s = state();
    let out = decode(
        0x0C,
        body(&ServerboundClientCommand {
            action: ClientCommandAction::PerformRespawn,
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(out, vec![PlayerInput::Respawn { connection: conn() }]);
    let out = decode(
        0x0C,
        body(&ServerboundClientCommand {
            action: ClientCommandAction::RequestStats,
        }),
        &mut s,
    )
    .unwrap();
    assert_eq!(out, vec![PlayerInput::RequestStats { connection: conn() }]);
}

#[test]
fn request_statistics_exact_javap_fixture_and_invalid_payloads() {
    // Local server-26.1.2.jar GameProtocols SB registration 12 and
    // ServerboundClientCommandPacket$Action REQUEST_STATS ordinal 1.
    assert_eq!(
        decode(0x0c, vec![1], &mut state()).unwrap(),
        vec![PlayerInput::RequestStats { connection: conn() }],
    );
    assert!(decode(0x0c, vec![1, 0], &mut state()).is_err());
    assert!(decode(0x0c, vec![3], &mut state()).is_err());
    assert!(decode(0x0c, Vec::new(), &mut state()).is_err());
    assert!(decode(0x0c, vec![2], &mut state()).unwrap().is_empty());
}

#[test]
fn ignore_list_returns_empty() {
    let mut s = state();
    let cases: Vec<(i32, Vec<u8>)> = vec![
        (
            ServerboundKeepAlive::ID,
            body(&ServerboundKeepAlive { id: 1 }),
        ),
        (
            ServerboundClientTickEnd::ID,
            body(&ServerboundClientTickEnd),
        ),
        (
            ServerboundChunkBatchReceived::ID,
            body(&ServerboundChunkBatchReceived {
                desired_chunks_per_tick: 1.0,
            }),
        ),
        (
            ServerboundClientInformation::ID,
            body(&ServerboundClientInformation {
                information: sample_client_information(),
            }),
        ),
        (
            ServerboundResourcePack::ID,
            body(&ServerboundResourcePack {
                status: ResourcePackStatus {
                    id: uuid::Uuid::nil(),
                    action: ResourcePackAction::SuccessfullyLoaded,
                },
            }),
        ),
        (
            ServerboundCustomPayload::ID,
            body(&ServerboundCustomPayload {
                payload: CustomPayload::Brand("v".to_owned()),
            }),
        ),
        (
            ServerboundChatAck::ID,
            body(&ServerboundChatAck { offset: 3 }),
        ),
        (
            ServerboundCommandSuggestion::ID,
            body(&mc_protocol::packets::play::ServerboundCommandSuggestion {
                id: 1,
                command: "k".to_owned(),
            }),
        ),
    ];
    for (id, bytes) in cases {
        let out = decode(id, bytes, &mut s).unwrap();
        assert!(out.is_empty(), "id {id:#04x} should ignore");
    }
}
#[test]
fn sprint_and_sign_updates_become_authoritative_inputs() {
    let mut s = state();
    for (action, sprinting) in [
        (PlayerCommandAction::StartSprinting, true),
        (PlayerCommandAction::StopSprinting, false),
    ] {
        assert_eq!(
            decode(
                ServerboundPlayerCommand::ID,
                body(&ServerboundPlayerCommand {
                    entity_id: 1,
                    action,
                    data: 0,
                }),
                &mut s
            )
            .unwrap(),
            vec![PlayerInput::Sprint {
                connection: conn(),
                sprinting
            }]
        );
    }
    for sprinting in [true, false] {
        assert_eq!(
            decode(
                ServerboundPlayerInput::ID,
                body(&ServerboundPlayerInput {
                    input: mc_protocol::packets::play::PlayerInput {
                        sprint: sprinting,
                        ..Default::default()
                    },
                }),
                &mut s,
            )
            .unwrap(),
            vec![
                PlayerInput::VehicleInput {
                    connection: conn(),
                    flags: if sprinting { 64 } else { 0 }
                },
                PlayerInput::Sneak {
                    connection: conn(),
                    sneaking: false,
                },
                PlayerInput::Sprint {
                    connection: conn(),
                    sprinting,
                },
            ],
        );
    }
    let lines = vec![
        "one".to_owned(),
        "two".to_owned(),
        "".to_owned(),
        "".to_owned(),
    ];
    assert_eq!(
        decode(
            ServerboundSignUpdate::ID,
            body(&ServerboundSignUpdate {
                position: pack_block_pos(1, 64, 2),
                lines: lines.clone(),
                is_front_text: true,
            }),
            &mut s
        )
        .unwrap(),
        vec![PlayerInput::SignUpdate {
            connection: conn(),
            position: BlockPos { x: 1, y: 64, z: 2 },
            front: true,
            lines,
        }]
    );
}

#[test]
fn sneak_and_creative_slot_become_inputs() {
    let mut s = state();
    let input = mc_protocol::packets::play::PlayerInput {
        shift: true,
        ..Default::default()
    };
    let out = decode(
        ServerboundPlayerInput::ID,
        body(&ServerboundPlayerInput { input }),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out.as_slice(),
        [
            PlayerInput::VehicleInput { flags: 32, .. },
            PlayerInput::Sneak { sneaking: true, .. },
            PlayerInput::Sprint {
                sprinting: false,
                ..
            }
        ]
    ));
    let stack = mc_data::item_stack::ItemStack::new(1, 5);
    let out = decode(
        0x38,
        body(
            &mc_protocol::packets::play::ServerboundSetCreativeModeSlot {
                slot: 36,
                item_stack: stack.clone(),
            },
        ),
        &mut s,
    )
    .unwrap();
    assert!(matches!(
        out.as_slice(),
        [PlayerInput::CreativeSlot { slot: 36, stack: got, .. }] if *got == stack
    ));
}

#[test]
fn stop_sleeping_becomes_leave_bed() {
    let mut s = state();
    let out = decode(
        ServerboundPlayerCommand::ID,
        body(&ServerboundPlayerCommand {
            entity_id: 1,
            action: PlayerCommandAction::StopSleeping,
            data: 0,
        }),
        &mut s,
    )
    .unwrap();
    assert!(matches!(out.as_slice(), [PlayerInput::LeaveBed { .. }]));
}

#[test]
fn trailing_bytes_rejected() {
    let mut s = state();
    let mut bytes = body(&ServerboundAttack { entity_id: 1 });
    bytes.push(0xFF);
    assert!(decode(ServerboundAttack::ID, bytes, &mut s).is_err());
}

#[test]
fn unknown_id_ignored_not_dropped() {
    // Forward compatibility: version-dependent client packets with
    // well-formed framing are acknowledge-and-ignore. Only malformed
    // KNOWN bodies are Err.
    let mut s = state();
    assert_eq!(decode(0x77, vec![], &mut s).unwrap(), vec![]);
    assert_eq!(decode(0x77, vec![1, 2, 3], &mut s).unwrap(), vec![]);
}

fn sample_client_information() -> ClientInformation {
    ClientInformation {
        language: "en_us".to_owned(),
        view_distance: 8,
        chat_visibility: ChatVisibility::Full,
        chat_colors: true,
        model_customisation: 127,
        main_hand: MainHand::Right,
        text_filtering_enabled: false,
        allows_listing: true,
        particle_status: ParticleStatus::All,
    }
}

#[test]
fn vehicle_move_and_secondary_interaction_preserve_authority_inputs() {
    let packet = mc_protocol::packets::play::ServerboundMoveVehicle {
        x: 1.0,
        y: 65.0,
        z: -3.0,
        yaw: 90.0,
        pitch: 4.0,
        on_ground: true,
    };
    assert_eq!(
        decode(0x22, body(&packet), &mut state()).unwrap(),
        vec![PlayerInput::MoveVehicle {
            connection: conn(),
            position: [1.0, 65.0, -3.0],
            yaw: 90.0,
            pitch: 4.0,
            on_ground: true,
        }]
    );
    let packet = ServerboundInteract {
        entity_id: 43,
        hand: InteractionHand::MainHand,
        location: mc_protocol::packets::play::EntityVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        using_secondary_action: true,
    };
    assert!(matches!(
        decode(ServerboundInteract::ID, body(&packet), &mut state())
            .unwrap()
            .as_slice(),
        [PlayerInput::InteractEntitySecondary {
            target: mc_core::boundary::EntityId(43),
            ..
        }]
    ));
}
