//! Leash authority is exercised through native Interact decoding and viewer wire plans.
use super::{CONNECTION, DecodeState, entity_slot, fixture, packet, stack};
use crate::plan::{ChunkPacketCache, PlannedPacket, ViewerState, plan_viewer};
use mc_core::boundary::{ConnectionId, EntityId, PlayerInput, TickOutput};
use mc_core::tick::{TickState, run_tick};
use mc_data::{Identifier, ItemStack};
use mc_protocol::frame::{Compression, try_decode_frame};
use mc_protocol::packets::Packet;
use mc_protocol::packets::play::{
    ClientboundSetEntityLeash, EntityVec3, InteractionHand, ServerboundInteract,
    ServerboundSetCarriedItem,
};

fn interact(target: EntityId, hand: InteractionHand, secondary: bool) -> ServerboundInteract {
    assert_eq!(
        ServerboundInteract::ID,
        0x1a,
        "26.1.2 GameProtocols ordinal"
    );
    ServerboundInteract {
        entity_id: target.0,
        hand,
        location: EntityVec3 {
            x: 0.0,
            y: 0.0,
            z: 0.0,
        },
        using_secondary_action: secondary,
    }
}
fn links(packets: &[PlannedPacket], target: EntityId) -> Vec<Option<i32>> {
    assert_eq!(
        ClientboundSetEntityLeash::ID,
        0x64,
        "26.1.2 GameProtocols ordinal"
    );
    packets
        .iter()
        .filter(|packet| packet.id == ClientboundSetEntityLeash::ID)
        .filter_map(|packet| {
            let mut frame = bytes::BytesMut::from(&packet.data[..]);
            let framed = try_decode_frame(&mut frame, Compression::Threshold(256))
                .unwrap()
                .unwrap();
            let mut body = framed.body;
            let original = body.clone();
            let link = ClientboundSetEntityLeash::decode(&mut body).unwrap();
            assert!(body.is_empty());
            if link.source_entity_id != target.0 {
                return None;
            }
            assert!(packet.reliable, "link updates must be reliable");
            let mut expected = target.0.to_be_bytes().to_vec();
            expected.extend_from_slice(&link.holder_entity_id.unwrap_or(0).to_be_bytes());
            assert_eq!(
                original.as_ref(),
                expected.as_slice(),
                "javap: two fixed BE ints; destination zero detaches"
            );
            Some(link.holder_entity_id)
        })
        .collect()
}
fn lead_drops(state: &TickState) -> i32 {
    let lead = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:lead").unwrap())
        .unwrap();
    state
        .entities
        .slots()
        .filter_map(|slot| state.entities.row(slot).item.as_ref())
        .filter(|item| item.item_id == lead)
        .map(|item| item.count)
        .sum()
}
fn attached() -> (
    TickState,
    DecodeState,
    EntityId,
    EntityId,
    ViewerState,
    ChunkPacketCache,
) {
    let (mut state, mut decoder, actor) = fixture();
    let pig_type = state
        .registry
        .entity_types
        .id_of(&Identifier::parse("minecraft:pig").unwrap())
        .unwrap();
    let pig = state.spawn_mob(pig_type, [10.5, 201.0, 8.5], 10.0);
    state
        .entities
        .row_mut(entity_slot(&state, pig))
        .controls
        .no_ai = true;
    let leads = stack(&state, "minecraft:lead", 2);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = leads;
    let mut viewer = ViewerState::new(CONNECTION, actor, [8.5, 201.0, 8.5]);
    let cache = ChunkPacketCache::new();
    let initial = run_tick(&mut state, &[]).0;
    plan_viewer(&initial, &mut viewer, 1, &cache);
    let output = packet(
        &mut state,
        &mut decoder,
        interact(pig, InteractionHand::MainHand, false),
    );
    assert!(
        state
            .entities
            .row(entity_slot(&state, pig))
            .entity_life
            .leash
            .is_some()
    );
    assert_eq!(
        state
            .entities
            .row(entity_slot(&state, pig))
            .appearance
            .leash_holder,
        Some(actor)
    );
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(lead_drops(&state), 0);
    assert_eq!(
        links(&plan_viewer(&output, &mut viewer, 1, &cache), pig),
        [Some(actor.0)]
    );
    (state, decoder, actor, pig, viewer, cache)
}
fn assert_detached(
    state: &TickState,
    output: &TickOutput,
    actor: EntityId,
    pig: EntityId,
    viewer: &mut ViewerState,
    cache: &ChunkPacketCache,
) {
    let row = state.entities.row(entity_slot(state, pig));
    assert!(row.entity_life.leash.is_none());
    assert_eq!(row.appearance.leash_holder, None);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(lead_drops(state), 1);
    assert_eq!(links(&plan_viewer(output, viewer, 1, cache), pig), [None]);
}

#[test]
fn leash_decode_normal_empty_hand_detaches_and_emits_empty_link_once() {
    let (mut state, mut decoder, actor, pig, mut viewer, cache) = attached();
    state.players.get_mut(&actor).unwrap().inventory.slots[2] = ItemStack::EMPTY;
    packet(
        &mut state,
        &mut decoder,
        ServerboundSetCarriedItem { slot: 2 },
    );
    let output = packet(
        &mut state,
        &mut decoder,
        interact(pig, InteractionHand::MainHand, false),
    );
    assert_detached(&state, &output, actor, pig, &mut viewer, &cache);
    let duplicate = packet(
        &mut state,
        &mut decoder,
        interact(pig, InteractionHand::MainHand, false),
    );
    assert_eq!(
        lead_drops(&state),
        1,
        "repeated Interact must not drop another lead"
    );
    assert!(links(&plan_viewer(&duplicate, &mut viewer, 1, &cache), pig).is_empty());
}

#[test]
fn leash_decode_empty_offhand_also_detaches_without_mainhand_debit() {
    let (mut state, mut decoder, actor, pig, mut viewer, cache) = attached();
    state.players.get_mut(&actor).unwrap().inventory.slots[mc_core::inventory::OFF_HAND] =
        ItemStack::EMPTY;
    let output = packet(
        &mut state,
        &mut decoder,
        interact(pig, InteractionHand::OffHand, false),
    );
    assert_detached(&state, &output, actor, pig, &mut viewer, &cache);
}

#[test]
fn leash_decode_nonholder_empty_hand_does_not_detach_or_drop() {
    let (mut state, _, actor, pig, mut viewer, cache) = attached();
    let other = ConnectionId(88);
    run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: other,
            uuid: uuid::Uuid::from_u128(88),
            name: "other-holder-probe".into(),
            permission_level: 0,
        }],
    );
    let other_actor = state.connections.entity_of(other).unwrap();
    state
        .entities
        .set_pos(entity_slot(&state, other_actor), [8.5, 201.0, 9.5]);
    state.players.get_mut(&other_actor).unwrap().inventory.slots[0] = ItemStack::EMPTY;
    let mut raw = Vec::new();
    interact(pig, InteractionHand::MainHand, false)
        .encode(&mut raw)
        .unwrap();
    let inputs = super::decode_play_frame_for(
        other,
        ServerboundInteract::ID,
        &raw,
        &mut DecodeState::default(),
    )
    .unwrap();
    let output = run_tick(&mut state, &inputs).0;
    assert!(
        state
            .entities
            .row(entity_slot(&state, pig))
            .entity_life
            .leash
            .is_some()
    );
    assert_eq!(
        state
            .entities
            .row(entity_slot(&state, pig))
            .appearance
            .leash_holder,
        Some(actor)
    );
    assert_eq!(lead_drops(&state), 0);
    assert!(links(&plan_viewer(&output, &mut viewer, 1, &cache), pig).is_empty());
}
