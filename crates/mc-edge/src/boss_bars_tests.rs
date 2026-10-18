use super::*;
use mc_core::boundary::{EntityAppearance, EntityId};
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::Packet;
use std::sync::Arc;
fn wither(health: f32, invulnerable: i32) -> EntityView {
    let type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:wither").unwrap())
        .unwrap();
    EntityView {
        id: EntityId(20),
        uuid: Uuid::from_u128(12),
        type_id,
        position: [0.0; 3],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: false,
        appearance: Arc::new(EntityAppearance {
            health,
            extra: vec![(19, EntityDataValue::VarInt(invulnerable))],
            ..Default::default()
        }),
    }
}
fn decode(packet: &PlannedPacket) -> BossOperation {
    assert!(packet.reliable);
    assert_eq!(packet.id, ClientboundBossEvent::ID);
    let mut bytes = bytes::BytesMut::from(packet.data.as_ref());
    let frame = try_decode_frame(&mut bytes, Compression::Threshold(256))
        .unwrap()
        .unwrap();
    assert!(bytes.is_empty());
    assert_eq!(frame.id, ClientboundBossEvent::ID);
    let mut body = frame.body.as_ref();
    let event = ClientboundBossEvent::decode(&mut body).unwrap();
    assert_eq!(event.uuid, Uuid::from_u128(12));
    assert!(body.is_empty());
    event.operation
}
#[test]
fn boss_baseline_add_update_remove_and_reentry_are_reliable() {
    let mut baseline = BossBaseline::default();
    let connection = ConnectionId(1);
    let mut packets = Vec::new();
    let entity = wither(100.0, 220);
    baseline.sync(connection, std::iter::once(&entity), &mut packets);
    assert!(matches!(
        decode(&packets[0]),
        BossOperation::Add {
            progress: 0.0,
            color: 5,
            overlay: 0,
            flags: 1,
            ..
        }
    ));
    packets.clear();
    baseline.sync(connection, std::iter::once(&entity), &mut packets);
    assert!(packets.is_empty());
    let entity = wither(100.0, 110);
    baseline.sync(connection, std::iter::once(&entity), &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Progress(0.5));
    packets.clear();
    let entity = wither(150.0, 0);
    baseline.sync(connection, std::iter::once(&entity), &mut packets);
    assert!(packets.is_empty());
    let dead = wither(0.0, 0);
    baseline.sync(connection, std::iter::once(&dead), &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Remove);
    packets.clear();
    baseline.sync(connection, std::iter::once(&entity), &mut packets);
    assert!(matches!(
        decode(&packets[0]),
        BossOperation::Add { progress: 0.5, .. }
    ));
    packets.clear();
    baseline.sync(connection, std::iter::empty(), &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Remove);
}

#[test]
fn dragon_bossbar_uses_pink_music_fog_and_survives_death_animation() {
    let mut dragon = wither(200.0, 0);
    dragon.type_id = mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse("minecraft:ender_dragon").unwrap())
        .unwrap();
    let mut baseline = BossBaseline::default();
    let mut packets = Vec::new();
    let connection = ConnectionId(1);
    baseline.sync(connection, std::iter::once(&dragon), &mut packets);
    assert!(matches!(
        decode(&packets[0]),
        BossOperation::Add {
            progress: 1.0,
            color: 0,
            overlay: 0,
            flags: 6,
            ..
        }
    ));
    packets.clear();
    Arc::make_mut(&mut dragon.appearance).health = 0.0;
    baseline.sync(connection, std::iter::once(&dragon), &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Progress(0.0));
    packets.clear();
    baseline.sync(connection, std::iter::empty(), &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Remove);
}

#[test]
fn raids_bar_add_health_victory_remove_and_reentry_are_snapshot_reliable() {
    let mut baseline = BossBaseline::default();
    let connection = ConnectionId(1);
    let mut packets = Vec::new();
    let mut bar = mc_core::raids::RaidBar {
        uuid: Uuid::from_u128(12),
        center: [0; 3],
        progress: 1.0,
        name: "event.minecraft.raid",
    };
    baseline.sync_raids(connection, [0.0; 3], &[bar.clone()], &mut packets);
    assert!(matches!(
        decode(&packets[0]),
        BossOperation::Add {
            progress: 1.0,
            color: 2,
            overlay: 2,
            flags: 0,
            ..
        }
    ));
    packets.clear();
    baseline.sync_raids(connection, [0.0; 3], &[bar.clone()], &mut packets);
    assert!(packets.is_empty());
    bar.progress = 0.25;
    baseline.sync_raids(connection, [0.0; 3], &[bar.clone()], &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Progress(0.25));
    packets.clear();
    bar.progress = 0.0;
    bar.name = "event.minecraft.raid.victory.full";
    baseline.sync_raids(connection, [0.0; 3], &[bar.clone()], &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Progress(0.0));
    assert!(matches!(decode(&packets[1]), BossOperation::Name(_)));
    packets.clear();
    baseline.sync_raids(connection, [1000.0; 3], &[bar.clone()], &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Remove);
    packets.clear();
    baseline.sync_raids(connection, [0.0; 3], &[bar], &mut packets);
    assert!(matches!(decode(&packets[0]), BossOperation::Add { .. }));
    packets.clear();
    baseline.sync_raids(connection, [0.0; 3], &[], &mut packets);
    assert_eq!(decode(&packets[0]), BossOperation::Remove);
}
