use super::registry_ids;
use mc_core::dimensions::Dimension;

#[test]
fn dimension_wire_ids_match_the_announced_registry() {
    let data = mc_data::solaris_required_data();
    let registry = data.registry("dimension_type").unwrap();
    let ids = registry_ids(&data).unwrap();
    for dimension in Dimension::ALL {
        assert_eq!(
            registry.entries[ids[dimension as usize] as usize].as_str(),
            dimension.name()
        );
    }
}

#[test]
fn incomplete_registry_never_falls_back_to_another_dimension() {
    let error = registry_ids(&mc_data::testing::stub()).unwrap_err();
    assert_eq!(
        error.to_string(),
        "dimension_type registry missing minecraft:overworld"
    );
}

use mc_core::boundary::{ConnectionId, EntityId, PlayerChange};
use mc_protocol::packets::Packet;
use std::sync::Arc;

fn body(packet: &crate::plan::PlannedPacket) -> bytes::Bytes {
    let mut framed = bytes::BytesMut::from(packet.data.as_ref());
    let frame = mc_protocol::frame::try_decode_frame(
        &mut framed,
        mc_protocol::frame::Compression::Threshold(256),
    )
    .unwrap()
    .unwrap();
    assert_eq!(frame.id, packet.id);
    assert!(framed.is_empty());
    frame.body
}

#[test]
fn travel_and_death_use_exact_registry_ids_and_distinct_data_retention() {
    use mc_protocol::packets::play::ClientboundRespawn;
    let data = mc_data::solaris_required_data();
    let ids = registry_ids(&data).unwrap();
    for dimension in Dimension::ALL {
        for death in [false, true] {
            let mut viewer =
                crate::plan::ViewerState::new(ConnectionId(7), EntityId(7), [8.0, 80.0, 8.0]);
            viewer.dimension_type_ids = ids;
            viewer.loaded_chunks.insert((0, 0), 1);
            viewer
                .tracked_entities
                .insert(EntityId(42), crate::plan::TrackedEntity::default());
            let change = if death {
                PlayerChange::Respawned {
                    teleport_id: 2,
                    dimension,
                    position: [8.0, 150.0, 8.0],
                    yaw: 12.0,
                    pitch: 3.0,
                    game_mode: 1,
                }
            } else {
                PlayerChange::DimensionChanged {
                    teleport_id: 2,
                    dimension,
                    position: [8.0, 150.0, 8.0],
                    yaw: 12.0,
                    pitch: 3.0,
                    game_mode: 1,
                    portal_cooldown: 10,
                }
            };
            let packets =
                crate::plan::plan_player_changes(std::slice::from_ref(&change), &mut viewer);
            let packet = packets
                .iter()
                .find(|packet| packet.id == ClientboundRespawn::ID)
                .unwrap();
            assert!(packet.reliable);
            let mut bytes = body(packet);
            let decoded = ClientboundRespawn::decode(&mut bytes).unwrap();
            assert!(bytes.is_empty());
            assert_eq!(decoded.portal_cooldown, if death { 0 } else { 10 });
            assert_eq!(decoded.dimension_type_id, ids[dimension as usize]);
            assert_eq!(decoded.dimension_name.as_str(), dimension.name());
            assert_eq!(
                decoded.sea_level,
                match dimension {
                    Dimension::Overworld => 63,
                    Dimension::Nether => 32,
                    Dimension::End => 0,
                }
            );
            assert_eq!(decoded.data_to_keep, if death { 0 } else { 3 });
            if !death || dimension != Dimension::Overworld {
                assert!(viewer.loaded_chunks.is_empty() && viewer.tracked_entities.is_empty());
            }
        }
    }
}

#[test]
fn global_roster_uses_player_info_update_and_remove_exact_codecs() {
    use mc_protocol::packets::play::{PlayerInfoActions, PlayerInfoRemove, PlayerInfoUpdate};
    let change = PlayerChange::Roster {
        players: Arc::from([mc_core::dimensions::RosterPlayer {
            connection: ConnectionId(8),
            entity: EntityId(81),
            uuid: uuid::Uuid::from_u128(8),
            name: "NetherPlayer".into(),
            game_mode: 1,
        }]),
        removed: Arc::from([uuid::Uuid::from_u128(9)]),
    };
    let mut viewer = crate::plan::ViewerState::new(ConnectionId(7), EntityId(7), [0.0; 3]);
    let packets = crate::plan::plan_player_changes(std::slice::from_ref(&change), &mut viewer);
    assert_eq!(packets.len(), 2);
    assert!(packets.iter().all(|packet| packet.reliable));
    let remove = PlayerInfoRemove::decode(&mut body(&packets[0])).unwrap();
    assert_eq!(remove.profile_ids, [uuid::Uuid::from_u128(9)]);
    let update = PlayerInfoUpdate::decode(&mut body(&packets[1])).unwrap();
    assert_eq!(update.actions, PlayerInfoActions::minimal_add_player());
    assert_eq!(update.entries[0].profile_id, uuid::Uuid::from_u128(8));
    assert_eq!(update.entries[0].name, "NetherPlayer");
    assert_eq!(update.entries[0].game_mode, 1);
    assert!(update.entries[0].listed);
}

#[test]
fn packet_cache_keeps_equal_coordinates_and_revisions_in_distinct_dimensions() {
    use crate::plan::{
        ChunkPacketCache, PlanRegistries, ViewerState, fill_chunk_packets, plan_viewer,
    };
    use mc_core::boundary::{SnapshotChunk, TickOutput, WorldSnapshot};
    use mc_protocol::packets::play::LevelChunkWithLight;
    let blocks =
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .unwrap();
    let items = mc_data::items::solaris_required_items();
    let block_entity_types = mc_data::block_entity_types::solaris_required_block_entity_types();
    let biomes = crate::chunks::test_biome_registry(&[
        "minecraft:plains",
        "minecraft:nether_wastes",
        "minecraft:the_end",
    ]);
    let registries = PlanRegistries {
        blocks: &blocks,
        items: &items,
        biomes: &biomes,
        block_entity_types: &block_entity_types,
    };
    let position = mc_world::ChunkPos { x: 0, z: 0 };
    let air = blocks
        .block(&mc_data::Identifier::parse("minecraft:air").unwrap())
        .unwrap()
        .default;
    let output = |dimension: Dimension, biome: &str| {
        let chunk = Arc::new(mc_world::Chunk::empty_with_geometry(
            position,
            air,
            mc_data::Identifier::parse(biome).unwrap(),
            dimension.geometry(),
        ));
        TickOutput {
            tick: 1,
            snapshot: Arc::new(WorldSnapshot {
                dimension,
                chunks: Arc::new(vec![SnapshotChunk {
                    pos: position,
                    revision: 1,
                    chunk,
                }]),
                ..Default::default()
            }),
            changes: Arc::new(Default::default()),
            times: Default::default(),
        }
    };
    let mut root = output(Dimension::Overworld, "minecraft:plains");
    for (dimension, biome) in [
        (Dimension::Nether, "minecraft:nether_wastes"),
        (Dimension::End, "minecraft:the_end"),
    ] {
        Arc::get_mut(&mut root.snapshot)
            .unwrap()
            .dimensions
            .insert(dimension, output(dimension, biome));
    }
    let mut cache = ChunkPacketCache::new();
    let mut plans = Vec::new();
    for (index, dimension) in Dimension::ALL.into_iter().enumerate() {
        let mut viewer = ViewerState::new(
            ConnectionId(index as u64 + 1),
            EntityId(index as i32 + 1),
            [8.0, 80.0, 8.0],
        );
        viewer.dimension = dimension;
        plans.push((
            viewer.connection,
            plan_viewer(&root, &mut viewer, 1, &cache),
        ));
    }
    assert!(fill_chunk_packets(&root, &mut plans, &registries, &mut cache).is_empty());
    let packets: Vec<_> = plans
        .iter()
        .map(|(_, packets)| {
            packets
                .iter()
                .find(|packet| packet.id == LevelChunkWithLight::ID)
                .unwrap()
        })
        .collect();
    assert_eq!(packets.len(), 3);
    assert!(packets.iter().all(|packet| !packet.data.is_empty()));
    assert_ne!(packets[0].data, packets[1].data);
    assert_ne!(packets[1].data, packets[2].data);
    cache.retain_resident(&root);
    for (index, dimension) in Dimension::ALL.into_iter().enumerate() {
        let mut viewer = ViewerState::new(
            ConnectionId(index as u64 + 11),
            EntityId(index as i32 + 11),
            [8.0, 80.0, 8.0],
        );
        viewer.dimension = dimension;
        let planned = plan_viewer(&root, &mut viewer, 1, &cache);
        let cached = planned
            .iter()
            .find(|packet| packet.id == LevelChunkWithLight::ID)
            .unwrap();
        assert_eq!(cached.data, packets[index].data);
        assert_eq!(cached.chunk_dimension, dimension);
    }
}
