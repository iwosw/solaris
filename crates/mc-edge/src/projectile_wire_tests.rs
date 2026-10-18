use super::*;
use mc_core::boundary::{ChangeSet, EntityAppearance, PlayerChange, TickTimes, WorldSnapshot};

fn body(packet: &PlannedPacket) -> bytes::Bytes {
    let mut framed = bytes::BytesMut::from(&packet.data[..]);
    mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
        .unwrap()
        .unwrap()
        .body
}

fn type_id(name: &str) -> u32 {
    mc_data::entity_types::solaris_required_entity_types()
        .id_of(&mc_data::Identifier::parse(name).unwrap())
        .unwrap()
}

#[test]
fn projectile_metadata_never_contains_living_accessors() {
    for name in [
        "arrow",
        "spectral_arrow",
        "trident",
        "snowball",
        "egg",
        "ender_pearl",
        // EntityType.java 26.1.2 registers SPLASH_POTION as splash_potion.
        "splash_potion",
        "lingering_potion",
        "experience_bottle",
        "firework_rocket",
        "fishing_bobber",
        "area_effect_cloud",
        "small_fireball",
        "fireball",
        "dragon_fireball",
        "wither_skull",
        "wind_charge",
        "breeze_wind_charge",
        "shulker_bullet",
        "llama_spit",
        "lightning_bolt",
        "oak_boat",
        "oak_chest_boat",
        "bamboo_raft",
        "bamboo_chest_raft",
        "minecart",
        "chest_minecart",
        "furnace_minecart",
        "hopper_minecart",
        "tnt_minecart",
    ] {
        let id = type_id(&format!("minecraft:{name}"));
        assert!(!has_living_metadata(id), "{name}");
        let mut packets = Vec::new();
        push_appearance_for(
            &mut packets,
            ConnectionId(7),
            EntityId(42),
            &EntityAppearance::default(),
            id,
            None,
        );
        let data = ClientboundSetEntityData::decode(&mut body(&packets[0])).unwrap();
        assert!(
            data.values.iter().all(|value| !matches!(
                value,
                WireEntityDataValue::Float {
                    index: LIVING_ENTITY_DATA_HEALTH_INDEX_26_1_2,
                    ..
                } | WireEntityDataValue::EffectParticles { .. }
                    | WireEntityDataValue::Boolean {
                        index: LIVING_ENTITY_DATA_EFFECT_AMBIENCE_INDEX,
                        ..
                    }
            )),
            "{name}"
        );
    }
    assert!(has_living_metadata(type_id("minecraft:player")));
    assert!(has_living_metadata(type_id("minecraft:cow")));
}

#[test]
fn cloud_particle_attachment_and_projectile_events_use_exact_wire_serializers() {
    let mut packets = Vec::new();
    let appearance = EntityAppearance {
        extra: vec![
            (8, mc_core::boundary::EntityDataValue::Float(2.5)),
            (9, mc_core::boundary::EntityDataValue::Boolean(false)),
            (
                10,
                mc_core::boundary::EntityDataValue::EntityEffectParticle(-1),
            ),
        ],
        ..Default::default()
    };
    push_appearance_for(
        &mut packets,
        ConnectionId(7),
        EntityId(42),
        &appearance,
        type_id("minecraft:area_effect_cloud"),
        None,
    );
    let data = ClientboundSetEntityData::decode(&mut body(&packets[0])).unwrap();
    assert!(
        data.values
            .contains(&WireEntityDataValue::EntityEffectParticle {
                index: 10,
                argb: -1
            })
    );
    assert_eq!(
        map_extra(
            9,
            &mc_core::boundary::EntityDataValue::OptionalUnsignedInt(None)
        ),
        WireEntityDataValue::OptionalUnsignedInt {
            index: 9,
            value: None
        }
    );
    let changes = [
        PlayerChange::ProjectileStatus {
            entity: EntityId(42),
            event: 17,
        },
        PlayerChange::ProjectileLevelEvent {
            position: mc_core::boundary::BlockPos {
                x: 1,
                y: 201,
                z: -2,
            },
            event: 2002,
            data: -13_083_194,
        },
    ];
    let mut viewer = ViewerState::new(ConnectionId(7), EntityId(7), [0.0; 3]);
    let packets = plan_player_changes(&changes, &mut viewer);
    assert!(packets.iter().all(|packet| packet.reliable));
    let event = WireEntityEvent::decode(&mut body(&packets[0])).unwrap();
    assert_eq!((event.entity_id, event.event_id), (42, 17));
    let level = mc_protocol::packets::play::LevelEvent::decode(&mut body(&packets[1])).unwrap();
    assert_eq!((level.event_id, level.data), (2002, -13_083_194));
}

#[test]
fn projectile_spawn_preserves_retained_owner_data() {
    let entity = EntityView {
        id: EntityId(42),
        uuid: uuid::Uuid::from_u128(42),
        type_id: type_id("minecraft:fishing_bobber"),
        position: [8.0, 201.0, 8.0],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: false,
        appearance: Arc::new(EntityAppearance {
            spawn_data: 123,
            ..Default::default()
        }),
    };
    let output = TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            entities: vec![entity].into(),
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: TickTimes::default(),
    };
    let mut viewer = ViewerState::new(ConnectionId(7), EntityId(7), [8.0, 201.0, 8.0]);
    viewer.loaded_chunks.insert((0, 0), 0);
    let packets = plan_viewer(&output, &mut viewer, 1, &ChunkPacketCache::new());
    let packet = packets
        .iter()
        .find(|packet| packet.id == AddEntity::ID)
        .unwrap();
    assert_eq!(AddEntity::decode(&mut body(packet)).unwrap().data, 123);
}
