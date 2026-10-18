//! Real held-use decoding -> accepted arrow impact -> living metadata publication.
use super::*;
use crate::plan::{ChunkPacketCache, PlannedPacket, ViewerState, plan_viewer};
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::play::{ClientboundSetEntityData, EntityDataValue};

fn counts(packets: &[PlannedPacket], target: EntityId) -> Vec<i32> {
    assert_eq!(
        ClientboundSetEntityData::ID,
        0x63,
        "26.1.2 packet report and GameProtocols ordinal"
    );
    packets
        .iter()
        .filter(|packet| packet.id == ClientboundSetEntityData::ID)
        .filter_map(|packet| {
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let frame = try_decode_frame(&mut framed, Compression::Threshold(256))
                .unwrap()
                .unwrap();
            assert!(framed.is_empty());
            let mut body = &frame.body[..];
            let metadata = ClientboundSetEntityData::decode(&mut body).unwrap();
            assert!(body.is_empty());
            if metadata.entity_id != target.0 {
                return None;
            }
            let value = metadata
                .values
                .iter()
                .find_map(|value| match value {
                    EntityDataValue::Int { index: 12, value } => Some(*value),
                    _ => None,
                })
                .expect("living DATA_ARROW_COUNT_ID must be present");
            assert!(metadata.values.contains(&EntityDataValue::Int {
                index: 13,
                value: 0
            }));
            // Independent javap oracle: unsigned-byte index 12, INT serializer 1,
            // VarInt payload; counts in this scenario are only zero or one.
            assert!(
                frame
                    .body
                    .windows(3)
                    .any(|bytes| bytes == [12, 1, value as u8])
            );
            assert_eq!(frame.body.last(), Some(&255));
            Some(value)
        })
        .collect()
}

fn bow_hit(
    refused: bool,
) -> (
    TickState,
    EntityId,
    ViewerState,
    ChunkPacketCache,
    TickOutput,
) {
    let (mut state, mut decoder, actor) = super::stream_tests::full_data_fixture();
    equip(&mut state, &mut decoder, actor, "minecraft:bow");
    let kind = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:iron_golem").unwrap())
        .unwrap();
    let target = state.spawn_mob(kind, [8.5, 201.0, 12.0], 100.0);
    let slot = entity_slot(&state, target);
    state.entities.row_mut(slot).controls.no_ai = true;
    state.entities.row_mut(slot).controls.invulnerable = refused;
    let mut viewer = ViewerState::new(CONNECTION, actor, [8.5, 201.0, 8.5]);
    let cache = ChunkPacketCache::new();
    let initial = run_tick(&mut state, &[]).0;
    assert_eq!(
        counts(&plan_viewer(&initial, &mut viewer, 1, &cache), target),
        [0]
    );
    start(&mut state, &mut decoder, InteractionHand::MainHand);
    hold(&mut state, 30);
    let mut output = release(&mut state, &mut decoder);
    for _ in 0..3 {
        if state
            .entities
            .row(entity_slot(&state, target))
            .appearance
            .arrow_count
            != 0
            || owned(&state, actor, ProjectileKind::Arrow).is_empty()
        {
            break;
        }
        output = run_tick(&mut state, &[]).0;
    }
    assert!(
        owned(&state, actor, ProjectileKind::Arrow).is_empty(),
        "arrow must have struck the authored target"
    );
    (state, target, viewer, cache, output)
}

#[test]
fn embedded_projectiles_decoded_bow_hit_publishes_one_and_vanilla_decay_zero() {
    let (mut state, target, mut viewer, cache, output) = bow_hit(false);
    let row = state.entities.row(entity_slot(&state, target));
    assert!(row.health < 100.0);
    assert_eq!(row.appearance.arrow_count, 1);
    assert_eq!(
        counts(&plan_viewer(&output, &mut viewer, 1, &cache), target),
        [1]
    );
    for _ in 0..579 {
        run_tick(&mut state, &[]);
        assert_eq!(
            state
                .entities
                .row(entity_slot(&state, target))
                .appearance
                .arrow_count,
            1
        );
    }
    let output = run_tick(&mut state, &[]).0;
    assert_eq!(
        state
            .entities
            .row(entity_slot(&state, target))
            .appearance
            .arrow_count,
        0
    );
    assert_eq!(
        counts(&plan_viewer(&output, &mut viewer, 1, &cache), target),
        [0]
    );
}

#[test]
fn embedded_projectiles_decoded_bow_refused_damage_does_not_embed() {
    let (state, target, mut viewer, cache, output) = bow_hit(true);
    let row = state.entities.row(entity_slot(&state, target));
    assert_eq!(row.health, 100.0);
    assert_eq!(row.appearance.arrow_count, 0);
    assert!(
        counts(&plan_viewer(&output, &mut viewer, 1, &cache), target)
            .iter()
            .all(|count| *count == 0)
    );
}

#[test]
fn embedded_projectiles_late_viewer_receives_current_embedded_count() {
    let (_, target, viewer, cache, output) = bow_hit(false);
    let mut late = ViewerState::new(CONNECTION, viewer.entity_id, [8.5, 201.0, 8.5]);
    assert_eq!(
        counts(&plan_viewer(&output, &mut late, 1, &cache), target),
        [1]
    );
}

#[test]
fn natural_skeleton_arrow_publishes_causal_damage_before_removal() {
    use mc_protocol::packets::play::{
        AddEntity, ClientboundDamageEvent, MoveEntityPos, MoveEntityPosRot, RemoveEntities,
    };
    let (mut state, _, player) = super::stream_tests::full_data_fixture();
    state.day_time = 18_000;
    state.difficulty = mc_core::survival::Difficulty::Normal;
    state.players.get_mut(&player).unwrap().vitals.food = 17;
    let position = [2.5, 201.0, 8.5];
    state
        .entities
        .set_pos(entity_slot(&state, player), position);
    let skeleton_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:skeleton").unwrap())
        .unwrap();
    let arrow_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:arrow").unwrap())
        .unwrap();
    let skeleton = state.spawn_mob(skeleton_type, [14.5, 201.0, 8.5], 20.0);
    let bow = stack(&state, "minecraft:bow", 1);
    Arc::make_mut(
        &mut state
            .entities
            .row_mut(entity_slot(&state, skeleton))
            .appearance,
    )
    .equipment[0] = Some(bow);
    let mut viewer = ViewerState::new(CONNECTION, player, position);
    let cache = ChunkPacketCache::new();
    let mut arrows = std::collections::HashSet::new();
    let mut moved = std::collections::HashSet::new();
    let mut hit = None;
    let mut removed = std::collections::HashSet::new();
    for _ in 0..400 {
        let output = run_tick(&mut state, &[]).0;
        for packet in plan_viewer(&output, &mut viewer, 1, &cache) {
            if !matches!(
                packet.id,
                AddEntity::ID
                    | MoveEntityPos::ID
                    | MoveEntityPosRot::ID
                    | ClientboundDamageEvent::ID
                    | RemoveEntities::ID
            ) {
                continue;
            }
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let frame = try_decode_frame(&mut framed, Compression::Threshold(256))
                .unwrap()
                .unwrap();
            let mut body = &frame.body[..];
            if packet.id == AddEntity::ID {
                let entity = AddEntity::decode(&mut body).unwrap();
                if entity.entity_type_id == arrow_type as i32 {
                    arrows.insert(entity.entity_id);
                    // Short-lived arrows can impact before their 20-tick
                    // update phase. Vanilla clients fly them from spawn velocity.
                    if entity.movement.x != 0.0
                        || entity.movement.y != 0.0
                        || entity.movement.z != 0.0
                    {
                        moved.insert(entity.entity_id);
                    }
                }
            } else if packet.id == MoveEntityPos::ID {
                moved.insert(MoveEntityPos::decode(&mut body).unwrap().entity_id);
            } else if packet.id == MoveEntityPosRot::ID {
                moved.insert(MoveEntityPosRot::decode(&mut body).unwrap().entity_id);
            } else if packet.id == ClientboundDamageEvent::ID {
                let damage = ClientboundDamageEvent::decode(&mut body).unwrap();
                if damage.entity_id == player.0 && damage.source_cause_id == skeleton.0 {
                    assert!(
                        arrows.contains(&damage.source_direct_id),
                        "hit follows received spawn"
                    );
                    assert!(
                        !removed.contains(&damage.source_direct_id),
                        "hit precedes removal"
                    );
                    hit = Some(damage.source_direct_id);
                }
            } else if packet.id == RemoveEntities::ID {
                removed.extend(RemoveEntities::decode(&mut body).unwrap().entity_ids);
            }
        }
        if hit.is_some_and(|id| removed.contains(&id)) {
            break;
        }
    }
    assert!(!arrows.is_empty(), "natural skeleton launches are received");
    assert!(
        state.players[&player].vitals.health < 20.0,
        "arrow damage applies"
    );
    let hit = hit.expect("clients receive causal skeleton arrow DamageEvent");
    assert!(
        moved.contains(&hit),
        "the hit arrow publishes flight motion before impact"
    );
    assert!(removed.contains(&hit), "the same hit arrow is removed");
}
