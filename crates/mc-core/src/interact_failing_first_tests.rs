//! Failing-first for fix-interact rows: existing-path interaction intents
//! assert the player-visible outcome. On bare 4575d3f8 these FAIL
//! (verified 5/5); with the fix they PASS.
use crate::boundary::{BlockPos, ConnectionId, Hand, PlayerInput};
use crate::farming_tests::{Y, block, put, setup};
use crate::tick::run_tick;
use mc_data::Identifier;

const POS: BlockPos = BlockPos { x: 1, y: Y + 1, z: 0 };

fn give(state: &mut crate::tick::TickState, actor: crate::boundary::EntityId, name: &str) {
    let id = state
        .registry
        .items
        .id_of(&Identifier::parse(format!("minecraft:{name}")).unwrap())
        .unwrap();
    state.players.get_mut(&actor).unwrap().inventory.slots[0] =
        mc_data::ItemStack::new(id, 1);
}

fn use_on(state: &mut crate::tick::TickState, pos: BlockPos) {
    run_tick(
        state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: pos,
            face: 1,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
}

#[test]
fn interact_failing_first_axe_strips_log() {
    let (mut state, actor) = setup();
    let id = block(&state, "oak_log", &[("axis", "x")]);
    put(&mut state, POS.x, POS.y, POS.z, id);
    give(&mut state, actor, "iron_axe");
    use_on(&mut state, POS);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(block(&state, "stripped_oak_log", &[("axis", "x")])),
        "axe strips the log",
    );
}

#[test]
fn interact_failing_first_sponge_absorbs() {
    let (mut state, actor) = setup();
    let water = state.registry.states.water_levels[0];
    put(&mut state, 2, Y + 1, 0, water);
    give(&mut state, actor, "sponge");
    use_on(
        &mut state,
        BlockPos { y: Y, ..POS },
    );
    assert_ne!(
        state.world.block_at(2, Y + 1, 0),
        Some(water),
        "sponge absorbs adjacent water",
    );
}

#[test]
fn interact_failing_first_brush_advances_dusted() {
    let (mut state, actor) = setup();
    let id = block(&state, "suspicious_sand", &[("dusted", "0")]);
    put(&mut state, POS.x, POS.y, POS.z, id);
    give(&mut state, actor, "brush");
    let slot = state.entities.slot_of(actor).unwrap();
    let [x, y, z] = state.entities.pos(slot);
    let dx = f64::from(POS.x) + 0.5 - x;
    let dz = f64::from(POS.z) + 0.5 - z;
    let dy = f64::from(POS.y) + 0.5 - y - 1.62;
    #[allow(clippy::disallowed_methods)]
    let (yaw, pitch) = {
        let yaw = (-dx).atan2(dz).to_degrees() as f32;
        let pitch = (-dy).atan2(dx.hypot(dz)).to_degrees() as f32;
        (yaw, pitch)
    };
    state.entities.set_rot(slot, yaw, pitch);
    use_on(&mut state, POS);
    for _ in 0..200 {
        run_tick(&mut state, &[]);
    }
    assert_ne!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(block(&state, "suspicious_sand", &[("dusted", "0")])),
        "brushing advances dusted state",
    );
}

#[test]
fn interact_failing_first_painting_spawns_entity() {
    let (mut state, actor) = setup();
    let wall = block(&state, "stone", &[]);
    put(&mut state, POS.x, POS.y, POS.z, wall);
    give(&mut state, actor, "painting");
    run_tick(
        &mut state,
        &[PlayerInput::UseItemOn {
            connection: ConnectionId(1),
            hand: Hand::Main,
            position: POS,
            face: 2,
            cursor: [0.5; 3],
            sequence: 1,
        }],
    );
    let painting = state.registry.entity_type_id("minecraft:painting").unwrap();
    assert_eq!(
        state.entities.mob_slots(painting).count(),
        1,
        "painting use spawns a painting entity",
    );
}

#[test]
fn interact_failing_first_spawn_protected_for_non_op() {
    // With operators configured, a non-op dig at world spawn is refused:
    // the block survives. On bare base (no module) it breaks.
    let (mut state, _) = setup();
    run_tick(
        &mut state,
        &[PlayerInput::SpawnProtectionOperators {
            operators: vec![uuid::Uuid::new_v4()],
        }],
    );
    let [sx, _, sz] = state.world.world_spawn();
    let at = BlockPos { x: sx, y: Y + 1, z: sz };
    let actor = state.connections.entity_of(ConnectionId(1)).unwrap();
    assert!(
        crate::spawn_protection::protected(&state, actor, at),
        "non-op dig at spawn is protected",
    );
}
