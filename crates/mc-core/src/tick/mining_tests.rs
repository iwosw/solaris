use super::*;
use crate::crafter::tests::{block, fixture};

const CONNECTION: ConnectionId = ConnectionId(1);
const POS: BlockPos = BlockPos { x: 8, y: 201, z: 8 };

fn setup(name: &str) -> (TickState, EntityId, u32) {
    let (mut state, actor) = fixture(None);
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Survival;
    state.players.get_mut(&actor).unwrap().awaiting_teleport = None;
    let current = block(&state, name, &[]);
    let mut registry = crate::registry::CoreRegistry::load(None).unwrap();
    let mut hardness = Vec::new();
    // Real vanilla hardness, independently supplied rather than bootstrap defaults.
    for (name, speed) in [
        ("stone", 1.5),
        ("obsidian", 50.0),
        ("bedrock", -1.0),
        ("torch", 0.0),
    ] {
        let id = block(&state, name, &[]);
        if hardness.len() <= id as usize {
            hardness.resize(id as usize + 1, -1.0);
        }
        hardness[id as usize] = speed;
    }
    let correct_tool = vec![false; hardness.len()];
    registry.mining =
        mc_data::block_mining::BlockMiningTable::from_arrays("fixture", hardness, correct_tool);
    state.registry = std::sync::Arc::new(registry);
    state.world.set_block(POS.x, POS.y, POS.z, current);
    state.world.fold_overlay();
    (state, actor, current)
}

fn input(action: DigAction) -> PlayerInput {
    PlayerInput::Dig {
        connection: CONNECTION,
        position: POS,
        action,
        sequence: 1,
    }
}

fn direct(state: &mut TickState, action: DigAction) -> (Vec<Intent>, ChangeSet) {
    let mut intents = Vec::new();
    let mut changes = ChangeSet::default();
    input::interact::dig(
        state,
        (&CONNECTION, &POS, &action, &1),
        &mut intents,
        &mut changes,
    );
    (intents, changes)
}

#[test]
fn survival_finish_without_start_never_plans_a_break() {
    for name in ["stone", "obsidian", "bedrock"] {
        let (mut state, actor, _) = setup(name);
        assert!(
            plan_client_build(&state, actor, &input(DigAction::Finish)).is_none(),
            "hook planner accepted {name}"
        );
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "direct accepted {name}"
        );
    }
}

#[test]
fn survival_start_then_immediate_finish_never_plans_a_break() {
    for name in ["stone", "obsidian", "bedrock"] {
        let (mut state, actor, _) = setup(name);
        assert!(direct(&mut state, DigAction::Start).0.is_empty());
        assert!(
            plan_client_build(&state, actor, &input(DigAction::Finish)).is_none(),
            "hook planner accepted early {name}"
        );
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "direct accepted early {name}"
        );
    }
}

/// Other domain tests exercise the finish transaction, not its elapsed input
/// ticks. Prime it through the real mining owner, without running unrelated AI.
pub(crate) fn ready_to_finish(state: &mut TickState, connection: ConnectionId, position: BlockPos) {
    let actor = state.connections.entity_of(connection).unwrap();
    if state.players[&actor].creative() {
        return;
    }
    let current = state
        .world
        .block_at(position.x, position.y, position.z)
        .unwrap();
    if state.registry.mining.destroy_speed(current).is_none() {
        std::sync::Arc::make_mut(&mut state.registry).mining = mc_data::block_mining::load(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("../../data/vanilla/reports/block_mining.json"),
        )
        .unwrap();
    }
    assert!(
        crate::effects::mining_progress(state, actor, current) > 0.0,
        "fixture must be mineable"
    );
    mining::start(state, actor, position, current);
    for _ in 0..100_000 {
        if mining::allows(state, actor, position, current, DigAction::Finish) {
            return;
        }
        state.tick += 1;
        mining::advance(state);
    }
    panic!("fixture mining did not finish");
}

#[test]
fn mining_server_ticks_admit_finish_and_commit_consumes_progress() {
    let (mut state, actor, stone) = setup("stone");
    run_tick(&mut state, &[input(DigAction::Start)]);
    assert_eq!(state.world.block_at(POS.x, POS.y, POS.z), Some(stone));
    for _ in 0..45 {
        run_tick(&mut state, &[]);
    }
    assert!(plan_client_build(&state, actor, &input(DigAction::Finish)).is_some());
    run_tick(&mut state, &[input(DigAction::Finish)]);
    assert_eq!(
        state.world.block_at(POS.x, POS.y, POS.z),
        Some(state.registry.states.air)
    );
    assert!(state.players[&actor].dig_progress.is_none());
}

#[test]
fn mining_bedrock_and_missing_hardness_stay_unbreakable_after_server_ticks() {
    for name in ["bedrock", "chest"] {
        let (mut state, actor, current) = setup(name);
        if name == "chest" {
            std::sync::Arc::make_mut(&mut state.registry).mining = Default::default();
        }
        direct(&mut state, DigAction::Start);
        for _ in 0..200 {
            state.tick += 1;
            mining::advance(&mut state);
        }
        assert!(plan_client_build(&state, actor, &input(DigAction::Finish)).is_none());
        let (intents, correction) = direct(&mut state, DigAction::Finish);
        assert!(intents.is_empty());
        assert_eq!(
            correction.blocks,
            vec![crate::boundary::BlockChange {
                position: POS,
                state: current
            }]
        );
    }
}

#[test]
fn mining_cancel_replacement_reach_teleport_and_packet_spam_invalidate_credit() {
    for invalidation in ["cancel", "replacement", "reach", "teleport", "spam"] {
        let (mut state, actor, _) = setup("stone");
        direct(&mut state, DigAction::Start);
        for _ in 0..50 {
            if invalidation == "spam" {
                direct(&mut state, DigAction::Start);
            } else {
                state.tick += 1;
                mining::advance(&mut state);
            }
        }
        match invalidation {
            "cancel" => {
                direct(&mut state, DigAction::Cancel);
            }
            "replacement" => {
                let obsidian = block(&state, "obsidian", &[]);
                state.world.set_block(POS.x, POS.y, POS.z, obsidian);
            }
            "reach" => {
                let slot = state.entities.slot_of(actor).unwrap();
                state.entities.set_pos(slot, [100.0, 201.0, 100.0]);
                mining::advance(&mut state);
                state.entities.set_pos(slot, [8.5, 201.0, 10.5]);
            }
            "teleport" => {
                let player = state.players.get_mut(&actor).unwrap();
                player.begin_teleport();
                player.awaiting_teleport = None;
            }
            "spam" => {}
            _ => unreachable!(),
        }
        assert!(
            client_build_refused(&state, actor, &input(DigAction::Finish)),
            "{invalidation}"
        );
        assert!(
            direct(&mut state, DigAction::Finish).0.is_empty(),
            "{invalidation}"
        );
    }
}

#[test]
fn mining_creative_and_zero_hardness_start_remain_instant() {
    let (mut state, actor, _) = setup("bedrock");
    state.players.get_mut(&actor).unwrap().game_mode = crate::commands::GameMode::Creative;
    assert!(plan_client_build(&state, actor, &input(DigAction::Start)).is_some());
    assert_eq!(direct(&mut state, DigAction::Start).0.len(), 1);
    let (mut state, actor, _) = setup("torch");
    assert!(plan_client_build(&state, actor, &input(DigAction::Start)).is_some());
    assert_eq!(direct(&mut state, DigAction::Start).0.len(), 1);
}

#[test]
fn mining_finish_packet_burst_and_tool_switch_do_not_credit_unelapsed_ticks() {
    let (mut state, actor, stone) = setup("stone");
    direct(&mut state, DigAction::Start);
    let mut pick = crate::crafter::tests::stack(&state, "diamond_pickaxe", 1);
    pick.enchantments.push(mc_data::ItemEnchantment {
        id: mc_data::Identifier::parse("minecraft:efficiency").unwrap(),
        level: 5,
    });
    state.players.get_mut(&actor).unwrap().inventory.slots[0] = pick;
    for _ in 0..100 {
        assert!(direct(&mut state, DigAction::Finish).0.is_empty());
        mining::advance(&mut state);
    }
    assert!(!mining::allows(
        &state,
        actor,
        POS,
        stone,
        DigAction::Finish
    ));
    let other = BlockPos {
        x: POS.x + 1,
        ..POS
    };
    ready_to_finish(&mut state, CONNECTION, POS);
    state.world.set_block(other.x, other.y, other.z, stone);
    assert!(!mining::allows(
        &state,
        actor,
        other,
        stone,
        DigAction::Finish
    ));
}
