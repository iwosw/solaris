use super::*;
use mc_protocol::packets::play::ClientboundCooldown;

#[test]
fn item_cooldown_uses_exact_identifier_packet_and_connection_custody() {
    let changes = [PlayerChange::ItemCooldown {
        group: "minecraft:chorus_fruit".into(),
        ticks: 20,
    }];
    let packets = plan_player_changes(&changes, &mut viewer_at(7, [0.0; 3]));
    assert_eq!(packets.len(), 1);
    assert!(packets[0].reliable);
    assert_eq!(packets[0].id, 0x16);
    let packet = ClientboundCooldown::decode(&mut body_of(&packets[0])).unwrap();
    assert_eq!(packet.cooldown_group.as_str(), "minecraft:chorus_fruit");
    assert_eq!(packet.duration, 20);
}
