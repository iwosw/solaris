//! Focused raw-protocol tests for [`super::plan_labels`]: real encoded and
//! decoded [`AddEntity`] / [`ClientboundSetEntityData`] / [`RemoveEntities`]
//! shapes for spawn, text content, metadata-only updates, removals, re-entry
//! and dimension switch.

use super::*;
use crate::plan::{ChunkPacketCache, PlannedPacket, ViewerState, plan_viewer};
use mc_core::boundary::{
    ChangeSet, ChunkPos, ConnectionId, EntityAppearance, EntityId, SnapshotChunk, TickOutput,
    WorldSnapshot,
};
use mc_core::dimensions::Dimension;
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{AddEntity, ClientboundSetEntityData, RemoveEntities};
use std::sync::Arc;

fn body_of(packet: &PlannedPacket) -> bytes::Bytes {
    let mut framed = bytes::BytesMut::from(&packet.data[..]);
    try_decode_frame(&mut framed, Compression::Threshold(256))
        .expect("planned frame decodes")
        .expect("planned frame is complete")
        .body
}

fn chunk_at(x: i32, z: i32) -> (ChunkPos, Arc<mc_world::chunk::Chunk>) {
    let mut chunk = mc_world::chunk::Chunk::empty(
        ChunkPos { x, z },
        mc_world::BlockStateId(0),
        mc_data::Identifier::parse("minecraft:plains").expect("static id"),
    );
    chunk.section_lights.fill(mc_world::chunk::SectionLight {
        block: Some(mc_world::chunk::LightSection::uniform(0)),
        sky: Some(mc_world::chunk::LightSection::uniform(0xFF)),
    });
    (ChunkPos { x, z }, Arc::new(chunk))
}

fn viewer_at(conn: u64, pos: [f64; 3]) -> ViewerState {
    ViewerState::new(ConnectionId(conn), EntityId(conn as i32), pos)
}

fn label(anchor: [i32; 3], text: &str) -> mc_core::settlements::presentation::SettlementLabel {
    mc_core::settlements::presentation::SettlementLabel {
        anchor,
        position: [
            f64::from(anchor[0]) + 0.5,
            f64::from(anchor[1]) + 2.6,
            f64::from(anchor[2]) + 0.5,
        ],
        text: text.into(),
    }
}

fn output(
    chunks: Vec<(ChunkPos, Arc<mc_world::chunk::Chunk>)>,
    labels: Vec<mc_core::settlements::presentation::SettlementLabel>,
    dimension: Dimension,
) -> TickOutput {
    static REVISION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(10_000);
    let mut chunks: Vec<SnapshotChunk> = chunks
        .into_iter()
        .map(|(pos, chunk)| SnapshotChunk {
            pos,
            revision: REVISION.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            chunk,
        })
        .collect();
    chunks.sort_by_key(|entry| (entry.pos.x, entry.pos.z));
    TickOutput {
        tick: 1,
        snapshot: Arc::new(WorldSnapshot {
            tick: 1,
            dimension,
            day_time: 1000,
            day_time_rate: 1.0,
            chunks: Arc::new(chunks),
            settlement_labels: Arc::new(labels),
            ..Default::default()
        }),
        changes: Arc::new(ChangeSet::default()),
        times: Default::default(),
    }
}

fn settle_chunks(viewer: &mut ViewerState, out: &TickOutput, _view_distance: i32) {
    // Fixture receipt: these chunks have already reached this viewer. Do not
    // consume planner label packets here: the test asserts those exact frames.
    for chunk in out.snapshot.chunks.iter() {
        viewer
            .loaded_chunks
            .insert((chunk.pos.x, chunk.pos.z), chunk.revision);
    }
    viewer.world_credit = 512;
}

fn label_packets(packets: &[PlannedPacket]) -> Vec<&PlannedPacket> {
    packets
        .iter()
        .filter(|packet| {
            packet.id == AddEntity::ID
                || packet.id == ClientboundSetEntityData::ID
                || packet.id == RemoveEntities::ID
        })
        .collect()
}

#[test]
fn settlement_labels_spawn_real_text_display_metadata() {
    let (pos, chunk) = chunk_at(0, 0);
    let out = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом · жильцов 1/2")],
        Dimension::Overworld,
    );
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    settle_chunks(&mut viewer, &out, 4);
    viewer.world_credit = 512;
    let cache = ChunkPacketCache::new();
    let packets = plan_viewer(&out, &mut viewer, 4, &cache);
    assert!(packets.iter().any(|packet| packet.id == AddEntity::ID));

    let spawn = packets
        .iter()
        .find(|packet| packet.id == AddEntity::ID)
        .expect("label spawn");
    assert!(spawn.reliable, "spawns ride the reliable queue");
    let added = AddEntity::decode(&mut &body_of(spawn)[..]).unwrap();
    assert!(
        added.entity_id < 0,
        "virtual ids never alias gameplay positives"
    );
    assert_eq!(
        added.entity_type_id, 135,
        "26.3 minecraft:text_display registry id;131 is stray"
    );
    assert!((added.x - 8.5).abs() < f64::EPSILON);
    assert!((added.y - 66.6).abs() < 1e-9);
    assert!((added.z - 8.5).abs() < f64::EPSILON);

    let metadata = packets
        .iter()
        .find(|packet| packet.id == ClientboundSetEntityData::ID)
        .expect("spawn metadata");
    let decoded = ClientboundSetEntityData::decode(&mut &body_of(metadata)[..]).unwrap();
    assert!(
        metadata.reliable,
        "spawn text must not be dropped independently of its entity"
    );
    assert_eq!(decoded.entity_id, added.entity_id);
    let values = super::label_spawn_metadata("Дом · жильцов 1/2");
    assert_eq!(
        decoded.values, values,
        "spawn carries the full billboard/view/shadow/text image"
    );
    assert!(
        decoded.values.iter().any(|value| matches!(
            value,
            WireEntityDataValue::Byte {
                index: 15,
                value: 3
            }
        )),
        "billboard CENTER per Display$BillboardConstraints javap oracle"
    );
    assert!(
        decoded.values.iter().any(
            |value| matches!(value, WireEntityDataValue::Component { index: 23, value } if value == "Дом · жильцов 1/2")
        ),
        "TEXT at TextDisplay index 23 with COMPONENT serializer id 5"
    );
    // Exact bytes: index 23, serializer id 5, then the literal text component NBT.
    let mut raw = Vec::new();
    decoded.encode(&mut raw).unwrap();
    assert!(raw.windows(2).any(|pair| pair == [23, 5]));
    assert_eq!(
        ClientboundSetEntityData::decode(&mut &raw[..]).unwrap(),
        decoded
    );
    // UUID is stable per label: same anchor and dimension re-derive it.
    assert_eq!(
        added.uuid,
        super::label_uuid(Dimension::Overworld, [8, 64, 8])
    );
}

#[test]
fn settlement_labels_update_only_on_text_change() {
    let (pos, chunk) = chunk_at(0, 0);
    let first = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом · жильцов 1/2")],
        Dimension::Overworld,
    );
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    settle_chunks(&mut viewer, &first, 4);
    viewer.world_credit = 512;
    let cache = ChunkPacketCache::new();
    let _ = plan_viewer(&first, &mut viewer, 4, &cache);

    // Same text again: no label traffic at all.
    let packets = plan_viewer(&first, &mut viewer, 4, &cache);
    assert!(
        label_packets(&packets).is_empty(),
        "steady labels emit no per-tick packets"
    );

    // Changed text: metadata-only, same virtual id, no new spawn or removal.
    // Same chunk list helper, but the live plan path below tolerates the new
    // fixture revision via the chunk-repair pass; no viewer reset needed.
    let (pos, chunk) = chunk_at(0, 0);
    let changed = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом · жильцов 2/2")],
        Dimension::Overworld,
    );
    let packets = plan_viewer(&changed, &mut viewer, 4, &cache);
    let updates: Vec<_> = packets
        .iter()
        .filter(|packet| packet.id == ClientboundSetEntityData::ID)
        .collect();
    assert_eq!(updates.len(), 1, "one metadata-only update");
    assert!(
        updates[0].reliable,
        "a text baseline advances only on reliable metadata"
    );
    let decoded = ClientboundSetEntityData::decode(&mut &body_of(updates[0])[..]).unwrap();
    assert_eq!(
        decoded.values,
        vec![WireEntityDataValue::Component {
            index: 23,
            value: "Дом · жильцов 2/2".into(),
        }]
    );
    assert!(
        !packets.iter().any(|packet| packet.id == AddEntity::ID),
        "retained anchor keeps its virtual id"
    );
    assert!(
        !packets.iter().any(|packet| packet.id == RemoveEntities::ID),
        "text change is not a removal"
    );
}

#[test]
fn settlement_labels_remove_on_exit_and_reenter_with_fresh_id() {
    let (pos, chunk) = chunk_at(0, 0);
    let present = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом · жильцов 1/2")],
        Dimension::Overworld,
    );
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    settle_chunks(&mut viewer, &present, 4);
    viewer.world_credit = 512;
    let cache = ChunkPacketCache::new();
    let spawned = plan_viewer(&present, &mut viewer, 4, &cache);
    let first_id = AddEntity::decode(
        &mut &body_of(
            spawned
                .iter()
                .find(|packet| packet.id == AddEntity::ID)
                .unwrap(),
        )[..],
    )
    .unwrap()
    .entity_id;

    // Label gone from the snapshot: reliable removal, id retired.
    let (pos, chunk) = chunk_at(0, 0);
    let gone = output(vec![(pos, chunk)], vec![], Dimension::Overworld);
    settle_chunks(&mut viewer, &gone, 4);
    let packets = plan_viewer(&gone, &mut viewer, 4, &cache);
    let removal = packets
        .iter()
        .find(|packet| packet.id == RemoveEntities::ID)
        .expect("exit removal");
    assert!(removal.reliable);
    assert_eq!(
        RemoveEntities::decode(&mut &body_of(removal)[..])
            .unwrap()
            .entity_ids,
        vec![first_id]
    );

    // Re-entry allocates a fresh virtual id: removed ids never alias.
    settle_chunks(&mut viewer, &present, 4);
    let packets = plan_viewer(&present, &mut viewer, 4, &cache);
    let second_id = AddEntity::decode(
        &mut &body_of(
            packets
                .iter()
                .find(|packet| packet.id == AddEntity::ID)
                .unwrap(),
        )[..],
    )
    .unwrap()
    .entity_id;
    assert!(second_id < 0);
    assert_ne!(second_id, first_id, "removed ids are never reused");
}

#[test]
fn settlement_labels_wait_for_chunk_and_leave_on_dimension_switch() {
    let (pos, chunk) = chunk_at(0, 0);
    let out = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом")],
        Dimension::Overworld,
    );
    let cache = ChunkPacketCache::new();
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    viewer.world_credit = 512;
    // Chunk not yet delivered: no floating label before its chunk.
    let packets = plan_viewer(&out, &mut viewer, 4, &cache);
    assert!(
        !packets.iter().any(|packet| packet.id == AddEntity::ID),
        "no label spawn before its chunk"
    );
    settle_chunks(&mut viewer, &out, 4);
    let packets = plan_viewer(&out, &mut viewer, 4, &cache);
    assert!(
        packets.iter().any(|packet| packet.id == AddEntity::ID),
        "spawn follows chunk admission"
    );

    // Dimension switch clears the virtual image reliably even though the new
    // projection still carries the same anchor (snapshot dimension differs).
    let (pos, chunk) = chunk_at(0, 0);
    let nether = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом")],
        Dimension::Nether,
    );
    viewer.dimension = Dimension::Nether;
    viewer.world_credit = 512;
    let packets = plan_viewer(&nether, &mut viewer, 4, &cache);
    let removals: Vec<_> = packets
        .iter()
        .filter(|packet| packet.id == RemoveEntities::ID)
        .collect();
    assert!(
        !removals.is_empty(),
        "dimension switch removes stale labels"
    );
    assert!(removals.iter().all(|packet| packet.reliable));
    for packet in &packets {
        if packet.id == RemoveEntities::ID {
            for id in RemoveEntities::decode(&mut &body_of(packet)[..])
                .unwrap()
                .entity_ids
            {
                assert!(id < 0);
            }
        }
    }
}

#[test]
fn settlement_labels_out_of_range_despawn_without_snapshot_change() {
    let (pos, chunk) = chunk_at(0, 0);
    let out = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом")],
        Dimension::Overworld,
    );
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    settle_chunks(&mut viewer, &out, 4);
    viewer.world_credit = 512;
    let cache = ChunkPacketCache::new();
    assert!(
        plan_viewer(&out, &mut viewer, 4, &cache)
            .iter()
            .any(|packet| packet.id == AddEntity::ID)
    );
    // Walk away: the label leaves visibility and is removed connection-locally.
    viewer.position = [200.5, 64.0, 200.5];
    viewer.loaded_chunks.insert((12, 12), 1);
    let packets = plan_viewer(&out, &mut viewer, 4, &cache);
    assert!(
        packets.iter().any(|packet| packet.id == RemoveEntities::ID),
        "out-of-range labels despawn without snapshot churn"
    );
    // Steady far state: no repeat removals.
    let packets = plan_viewer(&out, &mut viewer, 4, &cache);
    assert!(label_packets(&packets).is_empty());
}

#[test]
fn settlement_label_ids_never_alias_gameplay_positives() {
    // Ten churn cycles must stay strictly negative and non-repeating.
    let (pos, chunk) = chunk_at(0, 0);
    let present = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом")],
        Dimension::Overworld,
    );
    let (pos, chunk) = chunk_at(0, 0);
    let gone = output(vec![(pos, chunk)], vec![], Dimension::Overworld);
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    let cache = ChunkPacketCache::new();
    let mut seen = std::collections::HashSet::new();
    for _ in 0..10 {
        settle_chunks(&mut viewer, &present, 4);
        viewer.world_credit = 512;
        for packet in plan_viewer(&present, &mut viewer, 4, &cache) {
            if packet.id == AddEntity::ID {
                let id = AddEntity::decode(&mut &body_of(&packet)[..])
                    .unwrap()
                    .entity_id;
                assert!(id < 0, "virtual id {id} must stay negative");
                assert!(seen.insert(id), "virtual id {id} reused after removal");
            }
        }
        settle_chunks(&mut viewer, &gone, 4);
        let _ = plan_viewer(&gone, &mut viewer, 4, &cache);
    }
}

#[test]
fn settlement_labels_retained_updates_survive_zero_spawn_credit_and_stale_dimensions() {
    let (pos, chunk) = chunk_at(0, 0);
    let first = output(
        vec![(pos, chunk)],
        vec![label([8, 64, 8], "Дом 1")],
        Dimension::Overworld,
    );
    let mut viewer = viewer_at(1, [8.5, 64.0, 8.5]);
    settle_chunks(&mut viewer, &first, 4);
    let mut packets = Vec::new();
    plan_labels(&first.snapshot, &mut viewer, 4, &mut packets, &mut 512);
    assert_eq!(packets.len(), 2);
    let (pos, chunk) = chunk_at(0, 0);
    let changed = output(
        vec![(pos, chunk)],
        vec![label([1, 64, 1], "Новый дом"), label([8, 64, 8], "Дом 2")],
        Dimension::Overworld,
    );
    packets.clear();
    plan_labels(&changed.snapshot, &mut viewer, 4, &mut packets, &mut 0);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, ClientboundSetEntityData::ID);
    assert!(packets[0].reliable);
    viewer.dimension = Dimension::Nether;
    packets.clear();
    plan_labels(&changed.snapshot, &mut viewer, 4, &mut packets, &mut 512);
    assert_eq!(packets.len(), 1);
    assert_eq!(packets[0].id, RemoveEntities::ID);
    assert!(
        !packets.iter().any(|packet| packet.id == AddEntity::ID),
        "stale dimension must never create labels"
    );
}

#[test]
fn villager_custom_name_wire_uses_entity_boolean_index_three() {
    // Existing plan fixture shape (cf. `custom_name_visible_...` in plan_tests):
    // CustomName rides OptionalComponent index 2, visibility rides boolean index 3.
    let type_id = mc_data::entity_types::entity_type_contract_26_1_2_by_name("minecraft:villager")
        .unwrap()
        .protocol_id;
    let appearance = EntityAppearance {
        custom_name: Some("Иван · фермер · работа".into()),
        custom_name_visible: true,
        ..Default::default()
    };
    let mut packets = Vec::new();
    crate::plan::push_appearance_for(
        &mut packets,
        ConnectionId(1),
        EntityId(2),
        &appearance,
        type_id,
        None,
    );
    let data = packets
        .iter()
        .find(|packet| packet.id == ClientboundSetEntityData::ID)
        .unwrap();
    let decoded = ClientboundSetEntityData::decode(&mut body_of(data)).unwrap();
    assert!(
        decoded
            .values
            .contains(&WireEntityDataValue::OptionalComponent {
                index: 2,
                value: Some("Иван · фермер · работа".into()),
            })
    );
    assert!(decoded.values.contains(&WireEntityDataValue::Boolean {
        index: 3,
        value: true
    }));
    packets.clear();
    crate::plan::push_appearance_for(
        &mut packets,
        ConnectionId(1),
        EntityId(2),
        &appearance,
        type_id,
        Some(&appearance),
    );
    let packet = packets
        .iter()
        .find(|packet| packet.id == ClientboundSetEntityData::ID)
        .unwrap();
    let unchanged = ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap();
    assert!(
        !unchanged.values.iter().any(|value| matches!(
            value,
            WireEntityDataValue::OptionalComponent { index: 2, .. }
        )),
        "unchanged CustomName must not be resent alongside other metadata"
    );
    let mut resting = appearance.clone();
    resting.custom_name = Some("Иван · фермер · отдых".into());
    packets.clear();
    crate::plan::push_appearance_for(
        &mut packets,
        ConnectionId(1),
        EntityId(2),
        &resting,
        type_id,
        Some(&appearance),
    );
    let packet = packets
        .iter()
        .find(|packet| packet.id == ClientboundSetEntityData::ID)
        .unwrap();
    let delta = ClientboundSetEntityData::decode(&mut body_of(packet)).unwrap();
    assert!(
        delta
            .values
            .contains(&WireEntityDataValue::OptionalComponent {
                index: 2,
                value: resting.custom_name.clone(),
            })
    );
    assert!(
        !delta
            .values
            .iter()
            .any(|value| matches!(value, WireEntityDataValue::Boolean { index: 3, .. })),
        "unchanged visibility is not a name delta"
    );
}
