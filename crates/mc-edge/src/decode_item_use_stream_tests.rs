//! Multiple client packets form one authoritative server tick, not one tick per packet.
use super::*;
use mc_protocol::packets::play::{
    MovePlayerFlags, PlayerCommandAction, ServerboundClientTickEnd, ServerboundMovePlayerPosRot,
    ServerboundPlayerCommand, ServerboundPlayerInput,
};

pub(super) fn full_data_fixture() -> (TickState, DecodeState, EntityId) {
    static REGISTRY: LazyLock<Arc<CoreRegistry>> = LazyLock::new(|| {
        let directory =
            std::env::temp_dir().join(format!("item-use-stream-registry-{}", uuid::Uuid::new_v4()));
        let mut config = CoreConfig::new(directory.clone(), 42);
        config.view_distance = 0;
        config.simulation_distance = 0;
        config.checkpoint_every_ticks = u64::MAX;
        config.vanilla_data_dir =
            Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/vanilla"));
        let core = mc_core::Core::open(config).unwrap();
        let registry = Arc::clone(core.registry());
        core.shutdown();
        drop(core);
        std::fs::remove_dir_all(directory).unwrap();
        registry
    });
    fixture_with_registry(Arc::clone(&REGISTRY))
}

fn append<P: Packet>(decoder: &mut DecodeState, inputs: &mut Vec<PlayerInput>, packet: P) {
    let mut body = Vec::new();
    packet.encode(&mut body).unwrap();
    super::super::decode_play_frame(CONNECTION, P::ID, &body, decoder, inputs).unwrap();
}

fn client_tick(
    state: &mut TickState,
    decoder: &mut DecodeState,
    actor: EntityId,
    begin: bool,
    end: bool,
    repeat: bool,
) -> TickOutput {
    let mut inputs = Vec::new();
    append(
        decoder,
        &mut inputs,
        ServerboundPlayerInput {
            input: Default::default(),
        },
    );
    let position = state.entities.pos(entity_slot(state, actor));
    append(
        decoder,
        &mut inputs,
        ServerboundMovePlayerPosRot {
            x: position[0],
            y: position[1],
            z: position[2],
            yaw: 0.0,
            pitch: 0.0,
            flags: MovePlayerFlags::new(true, false),
        },
    );
    // Resending an unchanged selected slot must not cancel use either.
    append(
        decoder,
        &mut inputs,
        ServerboundSetCarriedItem {
            slot: i16::from(state.players[&actor].inventory.selected),
        },
    );
    append(
        decoder,
        &mut inputs,
        ServerboundPlayerCommand {
            entity_id: actor.0,
            action: PlayerCommandAction::StopSprinting,
            data: 0,
        },
    );
    if begin || repeat {
        append(
            decoder,
            &mut inputs,
            ServerboundUseItem {
                hand: InteractionHand::MainHand,
                sequence: state.tick as i32 + 1,
                y_rot: 0.0,
                x_rot: 0.0,
            },
        );
    }
    if end {
        append(
            decoder,
            &mut inputs,
            ServerboundPlayerAction {
                action: PlayerActionKind::ReleaseUseItem,
                position: pack_block_pos(0, 0, 0),
                direction: Direction::Down,
                sequence: 0,
            },
        );
    }
    append(decoder, &mut inputs, ServerboundClientTickEnd);
    run_tick(state, &inputs).0
}

fn command(state: &mut TickState, text: &str) {
    run_tick(
        state,
        &[PlayerInput::Command {
            connection: CONNECTION,
            permission_level: 4,
            command: text.into(),
        }],
    );
}

fn bow_stream(full_data: bool, repeated: bool) {
    let (mut state, mut decoder, actor) = if full_data {
        full_data_fixture()
    } else {
        fixture()
    };
    command(&mut state, "debug give minecraft:bow 1 0");
    command(&mut state, "debug give minecraft:arrow 4 1");
    client_tick(&mut state, &mut decoder, actor, true, false, false);
    for _ in 0..30 {
        client_tick(&mut state, &mut decoder, actor, false, false, repeated);
    }
    client_tick(&mut state, &mut decoder, actor, false, true, false);
    let arrows = owned(&state, actor, ProjectileKind::Arrow);
    assert_eq!(arrows.len(), 1);
    full_power_arrow(&state, arrows[0]);
    assert_eq!(state.players[&actor].inventory.slots[1].count, 3);
    assert_eq!(state.players[&actor].inventory.slots[0].damage, Some(1));
}

#[test]
fn decoded_stream_embedded_bow_single_start_survives_movement_and_tick_end() {
    bow_stream(false, false);
}
#[test]
fn decoded_stream_full_data_bow_single_start_survives_movement_and_tick_end() {
    bow_stream(true, false);
}
#[test]
fn decoded_stream_full_data_bow_repeated_starts_preserve_full_draw() {
    bow_stream(true, true);
}

#[test]
fn decoded_stream_full_data_golden_apple_given_by_debug_completes_at_full_hunger() {
    let (mut state, mut decoder, actor) = full_data_fixture();
    command(&mut state, "debug give minecraft:golden_apple 2 0");
    assert_eq!(state.players[&actor].vitals.food, 20);
    client_tick(&mut state, &mut decoder, actor, true, false, false);
    for _ in 0..39 {
        client_tick(&mut state, &mut decoder, actor, false, false, false);
    }
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    assert_eq!(
        state.players[&actor]
            .active_effects
            .get(EffectId::Regeneration)
            .unwrap()
            .amplifier,
        1
    );
    assert!(
        state.players[&actor]
            .active_effects
            .has(EffectId::Absorption)
    );
}

#[test]
fn decoded_stream_full_data_milk_given_by_debug_completes_with_bucket() {
    let (mut state, mut decoder, actor) = full_data_fixture();
    command(&mut state, "debug give minecraft:milk_bucket 1 0");
    state
        .players
        .get_mut(&actor)
        .unwrap()
        .active_effects
        .add(ActiveEffect::new(EffectId::Speed, 200, 0));
    client_tick(&mut state, &mut decoder, actor, true, false, false);
    for _ in 0..39 {
        client_tick(&mut state, &mut decoder, actor, false, false, false);
    }
    assert_eq!(
        state.players[&actor].inventory.slots[0],
        stack(&state, "minecraft:bucket", 1)
    );
    assert!(!state.players[&actor].active_effects.has(EffectId::Speed));
}

#[test]
fn decoded_stream_vanilla_elytra_command_starts_glide_then_boosts() {
    use mc_protocol::codec::WriteMc;
    let (mut state, mut decoder, actor) = fixture();
    run_tick(
        &mut state,
        &[PlayerInput::ConfirmTeleport {
            connection: CONNECTION,
            teleport_id: 1,
        }],
    );
    let elytra = stack(&state, "minecraft:elytra", 1);
    let rockets = stack(&state, "minecraft:firework_rocket", 2);
    let player = state.players.get_mut(&actor).unwrap();
    player.inventory.slots[38] = elytra;
    player.inventory.slots[0] = rockets;
    let slot = entity_slot(&state, actor);
    state.entities.set_pos(slot, [8.5, 241.0, 8.5]);
    state.entities.set_ground(slot, false);

    // Frozen 26.1.2 action ordinal, not our own enum encoder.
    let mut wire = Vec::new();
    wire.write_varint(actor.0);
    wire.extend([6, 0]);
    let inputs =
        super::super::decode_play_frame_for(CONNECTION, 0x2A, &wire, &mut decoder).unwrap();
    run_tick(&mut state, &inputs);
    assert_eq!(state.entities.row(slot).appearance.pose, 1);
    assert_eq!(
        state.entities.row(slot).appearance.shared_flags & 0x80,
        0x80
    );

    let mut inputs = Vec::new();
    append(
        &mut decoder,
        &mut inputs,
        ServerboundUseItem {
            hand: InteractionHand::MainHand,
            sequence: 1,
            y_rot: 0.0,
            x_rot: 0.0,
        },
    );
    run_tick(&mut state, &inputs);
    assert_eq!(state.players[&actor].inventory.slots[0].count, 1);
    let spawned = owned(&state, actor, ProjectileKind::Firework);
    assert_eq!(spawned.len(), 1);
    let rocket = state
        .entities
        .row(entity_slot(&state, spawned[0]))
        .projectile
        .as_ref()
        .unwrap();
    assert_eq!(rocket.attached_to, Some(state.entities.row(slot).uuid));
}
