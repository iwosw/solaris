use super::*;
use mc_core::commands::GameMode;
use mc_protocol::codec::WriteMc;
use mc_protocol::packets::play::ServerboundSetCreativeModeSlot;

#[test]
fn item_admission_decoded_unknown_creative_stack_produces_no_native_state_change() {
    let (mut state, mut decoder, actor) = super::stream_tests::full_data_fixture();
    state.players.get_mut(&actor).unwrap().game_mode = GameMode::Creative;
    let stone = stack(&state, "minecraft:stone", 3);
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = stone;
    let before = state.players[&actor].inventory.slots.clone();
    let count = state.entities.len();
    assert_eq!(ServerboundSetCreativeModeSlot::ID, 0x38);
    for id in [-1, i32::MIN, i32::MAX] {
        let mut body = Vec::new();
        body.write_i16(36);
        for raw in [1, id, 0, 0] {
            body.write_varint(raw);
        }
        assert!(
            decode_play_frame_for(
                CONNECTION,
                ServerboundSetCreativeModeSlot::ID,
                &body,
                &mut decoder
            )
            .is_err()
        );
        assert_eq!(state.players[&actor].inventory.slots, before);
        assert_eq!(state.entities.len(), count);
    }
    // Native producers receive the same fail-closed guard even without wire decoding.
    let output = run_tick(
        &mut state,
        &[PlayerInput::CreativeSlot {
            connection: CONNECTION,
            slot: 36,
            stack: ItemStack::new(u32::MAX, 1),
        }],
    )
    .0;
    assert_eq!(state.players[&actor].inventory.slots, before);
    assert!(output.changes.entities_spawned.is_empty());
}
