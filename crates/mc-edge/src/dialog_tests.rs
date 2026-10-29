//! Dialog edge path: core `ShowDialog`/`ClearDialog` -> packets, and serverbound
//! `custom_click_action` -> `PlayerInput::DialogClick`, with a fake connection.
use super::*;
use crate::decode::{DecodeState, decode_play_frame_for};
use mc_core::boundary::PlayerInput;
use mc_protocol::packets::play::{
    ClientboundClearDialog, ClientboundShowDialog, ServerboundCustomClickAction,
};

#[test]
fn show_and_clear_dialog_are_reliable_and_use_the_local_packet_ids() {
    let dialog = mc_nbt::Tag::Compound(vec![
        (
            "type".into(),
            mc_nbt::Tag::String("minecraft:notice".into()),
        ),
        ("title".into(), mc_nbt::Tag::String("Hello".into())),
    ]);
    let packets = plan_player_changes(
        &[
            PlayerChange::ShowDialog {
                dialog: dialog.clone(),
            },
            PlayerChange::ClearDialog,
        ],
        &mut viewer_at(7, [0.0; 3]),
    );
    assert_eq!(packets.len(), 2);
    assert!(packets.iter().all(|packet| packet.reliable));
    assert_eq!((packets[0].id, packets[1].id), (143, 142));
    let shown = ClientboundShowDialog::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(shown.dialog, dialog);
    assert!(body_of(&packets[1]).is_empty());
    let _ = ClientboundClearDialog::ID;
}

#[test]
fn custom_click_action_becomes_a_dialog_click_input_for_its_connection() {
    let payload = mc_nbt::Tag::Compound(vec![
        ("name".into(), mc_nbt::Tag::String("Ёлки".into())),
        ("radius".into(), mc_nbt::Tag::Float(12.0)),
    ]);
    let packet = ServerboundCustomClickAction {
        id: mc_protocol::codec::Identifier::parse("solaris:dialog_probe/submit").unwrap(),
        payload: Some(payload.clone()),
    };
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();
    let inputs = decode_play_frame_for(
        ConnectionId(7),
        ServerboundCustomClickAction::ID,
        &body,
        &mut DecodeState::default(),
    )
    .unwrap();
    assert_eq!(
        inputs,
        vec![PlayerInput::DialogClick {
            connection: ConnectionId(7),
            id: "solaris:dialog_probe/submit".into(),
            payload: Some(payload),
        }]
    );
    // A malformed body drops the connection like every other known packet.
    assert!(
        decode_play_frame_for(
            ConnectionId(7),
            ServerboundCustomClickAction::ID,
            &body[..body.len() - 1],
            &mut DecodeState::default()
        )
        .is_err()
    );
}
