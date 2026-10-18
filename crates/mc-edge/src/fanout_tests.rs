//! fanout.rs unit tests: priority tagging, framing, overflow rule.

use super::*;
use mc_core::boundary::{ChangeSet, TickOutput, WorldSnapshot};
use mc_protocol::packets::Packet;
use std::sync::Arc;

fn tick() -> TickOutput {
    TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            tick: 1,
            day_time: 0,
            chunks: Default::default(),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

#[test]
fn publish_tags_reliable_and_best_effort() {
    let out = tick();
    let mut v = ViewerState::new(
        ConnectionId(1),
        mc_core::boundary::EntityId(1),
        [0.5, 64.0, 0.5],
    );
    let packets = publish(&out, [&mut v].into_iter(), 0);
    assert!(!packets.is_empty());
    // Center/radius are reliable; all carry framed bytes.
    assert!(
        packets
            .iter()
            .any(|p| p.priority == Priority::Reliable && !p.data.is_empty())
    );
    for p in &packets {
        assert_eq!(p.connection, ConnectionId(1));
    }
}

#[test]
fn reliable_overflow_threshold() {
    let mk = |n: usize| -> Vec<AddressedPacket> {
        (0..n)
            .map(|i| AddressedPacket {
                connection: ConnectionId((i % 2) as u64 + 1),
                data: bytes::Bytes::from_static(b"x"),
                priority: Priority::Reliable,
            })
            .collect()
    };
    assert!(reliable_overflow(&mk(10)).is_empty());
    let victims = reliable_overflow(&mk(OUTBOUND_QUEUE_BOUND * 2 + 2));
    assert_eq!(victims.len(), 2);
    // Best-effort never overflows.
    let best: Vec<AddressedPacket> = (0..OUTBOUND_QUEUE_BOUND * 3)
        .map(|_| AddressedPacket {
            connection: ConnectionId(9),
            data: bytes::Bytes::from_static(b"x"),
            priority: Priority::BestEffort,
        })
        .collect();
    assert!(reliable_overflow(&best).is_empty());
}

#[test]
fn queue_bound_sane() {
    assert_eq!(OUTBOUND_QUEUE_BOUND, 512);
}

#[test]
fn publish_empty_viewers_empty_packets() {
    let out = tick();
    let packets: Vec<AddressedPacket> = publish(&out, [].into_iter(), 2);
    assert!(packets.is_empty());
}

fn registries_fixture() -> (
    mc_data::Registry,
    mc_world::BlockRegistry,
    mc_data::items::ItemRegistry,
    mc_data::block_entity_types::BlockEntityTypeRegistry,
) {
    (
        crate::chunks::test_biome_registry(&["minecraft:plains"]),
        mc_world::BlockRegistry::from_report(&mc_data::blocks::solaris_required_blocks_report())
            .expect("blocks build"),
        mc_data::items::solaris_required_items(),
        mc_data::block_entity_types::solaris_required_block_entity_types(),
    )
}

fn chunk_tick(
    revision: u64,
    chunk: Arc<mc_world::chunk::Chunk>,
    east: Option<(u64, Arc<mc_world::chunk::Chunk>)>,
) -> TickOutput {
    let mut chunks = vec![mc_core::boundary::SnapshotChunk {
        pos: mc_world::chunk::ChunkPos { x: 0, z: 0 },
        revision,
        chunk,
    }];
    if let Some((revision, chunk)) = east {
        chunks.push(mc_core::boundary::SnapshotChunk {
            pos: mc_world::chunk::ChunkPos { x: 1, z: 0 },
            revision,
            chunk,
        });
    }
    TickOutput {
        tick: revision,
        snapshot: Arc::new(WorldSnapshot {
            tick: revision,
            day_time: 0,
            chunks: std::sync::Arc::new(chunks),
            day_time_rate: 1.0,
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

/// With one cache across publishes, an unchanged revision serves the same
/// encoded chunk, and a changed chunk (new revision, no change set reached
/// the pump) is re-encoded with its new content, never served stale.
#[test]
fn cached_chunk_packets_follow_revisions() {
    let (biomes, blocks, items, block_entity_types) = registries_fixture();
    let registries = PlanRegistries {
        biomes: &biomes,
        blocks: &blocks,
        items: &items,
        block_entity_types: &block_entity_types,
    };
    let mut cache = crate::plan::ChunkPacketCache::new();
    let mut chunk_packet = |output: &TickOutput, viewer: u64| {
        let mut v = ViewerState::new(
            ConnectionId(viewer),
            mc_core::boundary::EntityId(viewer as i32),
            [8.0, 64.0, 8.0],
        );
        publish_with_registries(output, [&mut v].into_iter(), 0, &registries, &mut cache)
            .into_iter()
            .find(|p| {
                let mut framed = bytes::BytesMut::from(&p.data[..]);
                mc_protocol::frame::try_decode_frame(
                    &mut framed,
                    mc_protocol::frame::Compression::Threshold(256),
                )
                .ok()
                .flatten()
                .is_some_and(|frame| {
                    frame.id == mc_protocol::packets::play::LevelChunkWithLight::ID
                })
            })
            .expect("chunk packet routed")
            .data
    };
    let empty = |x| {
        let mut chunk = mc_world::chunk::Chunk::empty(
            mc_world::chunk::ChunkPos { x, z: 0 },
            mc_world::BlockStateId(0),
            mc_data::Identifier::parse("minecraft:plains").expect("static id"),
        );
        chunk.section_lights.fill(mc_world::chunk::SectionLight {
            block: Some(mc_world::chunk::LightSection::uniform(0)),
            sky: Some(mc_world::chunk::LightSection::uniform(0xFF)),
        });
        Arc::new(chunk)
    };
    let base = empty(0);
    let mut edited = (*base).clone();
    edited.set_block(4, 64, 4, mc_world::BlockStateId(1));
    let light_table = mc_data::block_light::load(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../data/vanilla/reports/block_light.json"
    ))
    .expect("block light report loads");
    let mut neighbours = [[None; 3]; 3];
    neighbours[1][1] = Some(&edited);
    let baked = mc_world::light::compute_chunk_light(neighbours, &light_table);
    edited.set_baked_light(&baked);
    let first = chunk_packet(&chunk_tick(1, Arc::clone(&base), None), 1);
    let repeat = chunk_packet(&chunk_tick(1, Arc::clone(&base), None), 2);
    let changed = chunk_packet(&chunk_tick(2, Arc::new(edited), None), 3);
    assert!(!first.is_empty());
    assert_eq!(first, repeat, "same revision, same packet");
    assert_ne!(first, changed, "new revision re-encoded, not served stale");
    // The core bakes neighbour relights and bumps the affected centre's
    // revision. Neighbour revisions alone no longer affect packet encoding.
    let glowstone = blocks
        .block(&mc_data::Identifier::parse("minecraft:glowstone").expect("id"))
        .expect("glowstone")
        .default;
    let mut lit = (*empty(1)).clone();
    lit.set_block(0, 64, 8, glowstone);
    let dark = chunk_packet(&chunk_tick(1, Arc::clone(&base), Some((10, empty(1)))), 4);
    let mut relit = (*base).clone();
    relit.section_lights[8].block = Some(mc_world::chunk::LightSection::uniform(1));
    let bright = chunk_packet(
        &chunk_tick(3, Arc::new(relit), Some((11, Arc::new(lit)))),
        5,
    );
    assert_ne!(dark, bright, "baked light and the centre revision changed");
}

#[test]
fn mooshroom_to_cow_fanout_removes_then_respawns_the_same_identity_for_every_viewer() {
    use mc_core::boundary::{EntityAppearance, EntityDataValue, EntityId, EntityView};
    use mc_protocol::frame::{Compression, try_decode_frame};
    use mc_protocol::packets::play::{AddEntity, ClientboundSetEntityData, RemoveEntities};
    let types = mc_data::entity_types::solaris_required_entity_types();
    let type_id = |name: &str| {
        types
            .id_of(&mc_data::Identifier::parse(name).unwrap())
            .unwrap()
    };
    let pos = mc_world::chunk::ChunkPos { x: 0, z: 0 };
    let mut chunk = mc_world::chunk::Chunk::empty(
        pos,
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    chunk.section_lights.fill(mc_world::chunk::SectionLight {
        block: Some(mc_world::chunk::LightSection::uniform(0)),
        sky: Some(mc_world::chunk::LightSection::uniform(0xFF)),
    });
    let mut output = chunk_tick(1, Arc::new(chunk), None);
    let uuid = uuid::Uuid::from_u128(0x1234);
    let resident = EntityView {
        id: EntityId(2),
        uuid,
        type_id: type_id("minecraft:mooshroom"),
        position: [8.0, 80.0, 8.0],
        velocity: [0.0; 3],
        yaw: 0.0,
        pitch: 0.0,
        head_yaw: 0.0,
        on_ground: true,
        appearance: Arc::new(EntityAppearance {
            extra: vec![(18, EntityDataValue::VarInt(1))],
            ..Default::default()
        }),
    };
    let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
    snapshot.entities = vec![resident].into();
    let mut viewers = [
        ViewerState::new(ConnectionId(1), EntityId(1), [8.0, 80.0, 8.0]),
        ViewerState::new(ConnectionId(3), EntityId(3), [8.0, 80.0, 8.0]),
    ];
    let first = publish(&output, viewers.iter_mut(), 0);
    for viewer in &viewers {
        assert!(viewer.tracked_entities.contains_key(&EntityId(2)));
        assert!(
            first
                .iter()
                .filter(|packet| packet.connection == viewer.connection)
                .any(|packet| {
                    let mut bytes = bytes::BytesMut::from(packet.data.as_ref());
                    let frame = try_decode_frame(&mut bytes, Compression::Threshold(256))
                        .unwrap()
                        .unwrap();
                    frame.id == AddEntity::ID
                        && AddEntity::decode(&mut frame.body.clone())
                            .unwrap()
                            .entity_type_id
                            == type_id("minecraft:mooshroom") as i32
                })
        );
    }
    output.tick = 2;
    let snapshot = Arc::get_mut(&mut output.snapshot).unwrap();
    snapshot.tick = 2;
    let mut rows: Vec<EntityView> = snapshot.entities.iter().cloned().collect();
    rows[0].type_id = type_id("minecraft:cow");
    Arc::make_mut(&mut rows[0].appearance).extra.clear();
    snapshot.entities = rows.into();
    // Core changed type in place; same-id removal/spawn tombstones are absent.
    Arc::get_mut(&mut output.changes)
        .unwrap()
        .entities_appearance_changed
        .push(EntityId(2));
    let packets = publish(&output, viewers.iter_mut(), 0);
    for viewer in &viewers {
        let frames: Vec<_> = packets
            .iter()
            .filter(|packet| packet.connection == viewer.connection)
            .map(|packet| {
                let mut bytes = bytes::BytesMut::from(packet.data.as_ref());
                let frame = try_decode_frame(&mut bytes, Compression::Threshold(256))
                    .unwrap()
                    .unwrap();
                if matches!(
                    frame.id,
                    AddEntity::ID | RemoveEntities::ID | ClientboundSetEntityData::ID
                ) {
                    assert_eq!(packet.priority, Priority::BestEffort);
                }
                frame
            })
            .collect();
        let removal = frames
            .iter()
            .position(|frame| frame.id == RemoveEntities::ID)
            .unwrap();
        let spawn = frames
            .iter()
            .position(|frame| frame.id == AddEntity::ID)
            .unwrap();
        let metadata = frames
            .iter()
            .position(|frame| frame.id == ClientboundSetEntityData::ID)
            .unwrap();
        assert!(removal < spawn && spawn < metadata);
        assert_eq!(
            RemoveEntities::decode(&mut frames[removal].body.clone())
                .unwrap()
                .entity_ids,
            [2]
        );
        let added = AddEntity::decode(&mut frames[spawn].body.clone()).unwrap();
        assert_eq!(added.entity_id, 2);
        assert_eq!(added.uuid, uuid);
        assert_eq!(added.entity_type_id, type_id("minecraft:cow") as i32);
        let data = ClientboundSetEntityData::decode(&mut frames[metadata].body.clone()).unwrap();
        assert_eq!(data.entity_id, 2);
        assert!(
            !data.values.iter().any(|value| matches!(
                value,
                mc_protocol::packets::play::EntityDataValue::Int { index: 18, .. }
            )),
            "mooshroom variant must not leak into the cow"
        );
        assert!(viewer.tracked_entities.contains_key(&EntityId(2)));
    }
    let repeat = publish(&output, viewers.iter_mut(), 0);
    assert!(repeat.iter().all(|packet| {
        let mut bytes = bytes::BytesMut::from(packet.data.as_ref());
        let frame = try_decode_frame(&mut bytes, Compression::Threshold(256))
            .unwrap()
            .unwrap();
        frame.id != RemoveEntities::ID && frame.id != AddEntity::ID
    }));
}
