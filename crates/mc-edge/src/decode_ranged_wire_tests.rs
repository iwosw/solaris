//! Decoded held-use packets publish legal inventory stacks in exact vanilla framing.
use super::*;
use crate::plan::{ViewerState, plan_player_changes};
use mc_protocol::codec::ReadMc;
use mc_protocol::frame::Compression;
use mc_protocol::packets::play::{ClientboundContainerSetContent, ClientboundContainerSetSlot};

fn guard_stack(state: &TickState, stack: &ItemStack) {
    if stack.is_empty() {
        assert_eq!(stack.count, 0);
        return;
    }
    let max = state.registry.item_rules().max_stack(stack);
    assert!(
        (1..=max).contains(&stack.count),
        "illegal count {} for item {} (max {max})",
        stack.count,
        stack.item_id
    );
    for projectile in &stack.extended().charged_projectiles {
        guard_stack(state, projectile);
    }
}

fn raw_stack(state: &TickState, bytes: &mut &[u8], template: bool, depth: usize) {
    assert!(depth <= 8);
    // Independent vanilla oracle, not the server's ItemStack decoder: the
    // template swaps the first two fields and cannot encode an empty stack.
    let (item, count) = if template {
        (
            bytes.read_varint().unwrap() as u32,
            bytes.read_varint().unwrap(),
        )
    } else {
        let count = bytes.read_varint().unwrap();
        if count == 0 {
            return;
        }
        (bytes.read_varint().unwrap() as u32, count)
    };
    guard_stack(state, &ItemStack::new(item, count));
    let added = bytes.read_varint().unwrap();
    let removed = bytes.read_varint().unwrap();
    for _ in 0..added {
        match bytes.read_varint().unwrap() {
            2 | 3 => {
                bytes.read_varint().unwrap();
            }
            13 => {
                let count = bytes.read_varint().unwrap();
                for _ in 0..count {
                    bytes.read_varint().unwrap();
                    bytes.read_varint().unwrap();
                }
            }
            51 => {
                let count = bytes.read_varint().unwrap();
                assert!((1..=3).contains(&count));
                for _ in 0..count {
                    raw_stack(state, bytes, true, depth + 1);
                }
            }
            id => panic!("unexpected component {id} in focused ranged fixture"),
        }
    }
    for _ in 0..removed {
        bytes.read_varint().unwrap();
    }
}

fn checked_tick(
    state: &mut TickState,
    actor: EntityId,
    inputs: &[PlayerInput],
) -> Vec<ClientboundContainerSetContent> {
    let (_, outboxes, _) = run_tick(state, inputs);
    let mut viewer = ViewerState::new(CONNECTION, actor, [8.5, 201.0, 8.5]);
    let mut images = Vec::new();
    for (connection, changes) in outboxes {
        assert_eq!(connection, CONNECTION);
        for packet in plan_player_changes(&changes, &mut viewer) {
            if packet.id != ClientboundContainerSetContent::ID
                && packet.id != ClientboundContainerSetSlot::ID
            {
                continue;
            }
            let mut framed = bytes::BytesMut::from(&packet.data[..]);
            let frame =
                mc_protocol::frame::try_decode_frame(&mut framed, Compression::Threshold(256))
                    .unwrap()
                    .unwrap();
            assert!(framed.is_empty());
            let mut raw = &frame.body[..];
            raw.read_varint().unwrap(); // container id
            raw.read_varint().unwrap(); // state id
            if packet.id == ClientboundContainerSetContent::ID {
                let count = raw.read_varint().unwrap();
                assert!((0..=256).contains(&count));
                for _ in 0..count {
                    raw_stack(state, &mut raw, false, 0);
                }
                raw_stack(state, &mut raw, false, 0);
                let image = ClientboundContainerSetContent::decode(&mut &frame.body[..]).unwrap();
                for stack in image
                    .items
                    .iter()
                    .chain(std::iter::once(&image.carried_item))
                {
                    guard_stack(state, stack);
                }
                images.push(image);
            } else {
                raw.read_i16().unwrap(); // slot
                raw_stack(state, &mut raw, false, 0);
                let slot = ClientboundContainerSetSlot::decode(&mut &frame.body[..]).unwrap();
                guard_stack(state, &slot.item_stack);
            }
            assert!(raw.is_empty(), "ranged packet must have no trailing bytes");
        }
    }
    for stack in &state.players[&actor].inventory.slots {
        guard_stack(state, stack);
    }
    for slot in state.entities.slots() {
        if let Some(projectile) = &state.entities.row(slot).projectile {
            guard_stack(state, &projectile.stack);
        }
    }
    images
}

fn decoded<P: Packet>(
    state: &mut TickState,
    decoder: &mut DecodeState,
    actor: EntityId,
    packet: P,
) -> Vec<ClientboundContainerSetContent> {
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();
    let inputs = decode_play_frame_for(CONNECTION, P::ID, &body, decoder).unwrap();
    assert!(!inputs.is_empty());
    checked_tick(state, actor, &inputs)
}

fn begin(
    state: &mut TickState,
    decoder: &mut DecodeState,
    actor: EntityId,
) -> Vec<ClientboundContainerSetContent> {
    decoded(
        state,
        decoder,
        actor,
        ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 1,
            y_rot: 0.0,
            x_rot: 0.0,
        },
    )
}

fn end(
    state: &mut TickState,
    decoder: &mut DecodeState,
    actor: EntityId,
) -> Vec<ClientboundContainerSetContent> {
    decoded(
        state,
        decoder,
        actor,
        ServerboundPlayerAction {
            action: PlayerActionKind::ReleaseUseItem,
            position: pack_block_pos(0, 0, 0),
            direction: Direction::Down,
            sequence: 2,
        },
    )
}

fn give(state: &mut TickState, actor: EntityId, name: &str, count: i32, slot: usize) {
    checked_tick(
        state,
        actor,
        &[PlayerInput::Command {
            connection: CONNECTION,
            permission_level: 4,
            command: format!("debug give {name} {count} {slot}"),
        }],
    );
    assert_eq!(
        state.players[&actor].inventory.slots[slot],
        stack(state, name, count)
    );
}

fn crossbow(multishot: bool) {
    let (mut state, mut decoder, actor) = super::stream_tests::full_data_fixture();
    give(&mut state, actor, "minecraft:crossbow", 1, 0);
    give(&mut state, actor, "minecraft:arrow", 1, 1);
    if multishot {
        state.players.get_mut(&actor).unwrap().inventory.slots[0] =
            stack(&state, "minecraft:crossbow", 1)
                .with_enchantment(Identifier::parse("minecraft:multishot").unwrap(), 1);
    }
    let arrow_id = stack(&state, "minecraft:arrow", 1).item_id;
    assert_eq!(state.players[&actor].inventory.slots[1].item_id, arrow_id);
    begin(&mut state, &mut decoder, actor);
    for _ in 1..25 {
        checked_tick(&mut state, actor, &[]);
    }
    let images = end(&mut state, &mut decoder, actor);
    assert!(
        !images.is_empty(),
        "decoded crossbow release must publish packet 0x12"
    );
    let count = if multishot { 3 } else { 1 };
    let image_weapon = images
        .iter()
        .flat_map(|image| &image.items)
        .find(|stack| !stack.extended().charged_projectiles.is_empty())
        .unwrap();
    assert_eq!(image_weapon.count, 1);
    assert_eq!(image_weapon.extended().charged_projectiles.len(), count);
    assert!(
        image_weapon
            .extended()
            .charged_projectiles
            .iter()
            .all(|stack| stack.item_id == arrow_id && stack.count == 1)
    );
    assert!(state.players[&actor].inventory.slots[1].is_empty());
    assert!(owned(&state, actor, ProjectileKind::Arrow).is_empty());
    begin(&mut state, &mut decoder, actor);
    assert_eq!(owned(&state, actor, ProjectileKind::Arrow).len(), count);
    assert!(
        state.players[&actor].inventory.slots[0]
            .extended()
            .charged_projectiles
            .is_empty()
    );
    assert_eq!(
        state.players[&actor].inventory.slots[0].damage,
        Some(if multishot { 3 } else { 1 })
    );
}

#[test]
fn ranged_wire_decoded_crossbow_one_arrow_uses_template_order_and_debits_once() {
    crossbow(false);
}

#[test]
fn ranged_wire_decoded_multishot_crossbow_has_three_count_one_templates() {
    crossbow(true);
}

#[test]
fn ranged_wire_decoded_bow_draw_release_keeps_all_encoded_counts_legal() {
    let (mut state, mut decoder, actor) = super::stream_tests::full_data_fixture();
    give(&mut state, actor, "minecraft:bow", 1, 0);
    give(&mut state, actor, "minecraft:arrow", 4, 1);
    begin(&mut state, &mut decoder, actor);
    for _ in 1..30 {
        checked_tick(&mut state, actor, &[]);
    }
    assert!(!end(&mut state, &mut decoder, actor).is_empty());
    let arrows = owned(&state, actor, ProjectileKind::Arrow);
    assert_eq!(arrows.len(), 1);
    full_power_arrow(&state, arrows[0]);
    assert_eq!(state.players[&actor].inventory.slots[1].count, 3);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
}

#[test]
fn ranged_wire_decoded_trident_release_keeps_inventory_and_projectile_counts_legal() {
    let (mut state, mut decoder, actor) = super::stream_tests::full_data_fixture();
    give(&mut state, actor, "minecraft:trident", 1, 0);
    begin(&mut state, &mut decoder, actor);
    for _ in 1..10 {
        checked_tick(&mut state, actor, &[]);
    }
    assert!(!end(&mut state, &mut decoder, actor).is_empty());
    let tridents = owned(&state, actor, ProjectileKind::Trident);
    assert_eq!(tridents.len(), 1);
    assert!(state.players[&actor].inventory.slots[0].is_empty());
    assert_eq!(
        state
            .entities
            .row(entity_slot(&state, tridents[0]))
            .projectile
            .as_ref()
            .unwrap()
            .stack
            .count,
        1
    );
}
