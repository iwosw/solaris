use super::*;
#[test]
fn frame_spawn_uses_attached_cell_instead_of_render_center() {
    for name in ["minecraft:item_frame", "minecraft:glow_item_frame"] {
        let mut frame = entity(7, [-1.96875, 65.5, 8.5]);
        frame.type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name(name)
            .unwrap()
            .protocol_id;
        assert_eq!(entity_spawn_position(&frame), [-2.0, 65.0, 8.0]);
    }
}
#[test]
fn signed_book_open_is_reliable_hand_packet() {
    let mut viewer = viewer_at(1, [8.0, 64.0, 8.0]);
    let packets = plan_player_changes(&[PlayerChange::BookOpened { hand: Hand::Off }], &mut viewer);
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(
        packets[0].id,
        mc_protocol::packets::play::ClientboundOpenBook::ID
    );
    assert_eq!(body_of(&packets[0]).as_ref(), &[1]);
}
