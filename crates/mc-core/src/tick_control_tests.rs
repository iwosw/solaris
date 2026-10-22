use super::*;
use crate::boundary::{ChangeSet, ConnectionId, PlayerInput};
use crate::command_tick::TickCommand;
use crate::horde_bench::flat_world;
use crate::tick::run_tick;
use std::sync::Arc;

const REQUESTER: ConnectionId = ConnectionId(7);

#[test]
fn default_manager_runs_at_twenty_ticks_per_second() {
    let mut control = TickControl::default();
    assert_eq!(control.rate(), 20.0);
    assert_eq!(control.period(), Duration::from_millis(50));
    assert!(control.begin_tick().is_none());
    assert!(control.runs_normally());
}

#[test]
fn rate_changes_the_period_and_never_drops_below_one() {
    let mut control = TickControl::default();
    control.set_rate(40.0);
    assert_eq!(control.period(), Duration::from_millis(25));
    control.set_rate(0.1);
    assert_eq!(control.rate(), 1.0);
    assert_eq!(control.period(), Duration::from_secs(1));
}

#[test]
fn frozen_ticks_do_not_simulate_until_stepped() {
    let mut control = TickControl::default();
    assert!(!control.step(3), "only a frozen game steps");
    control.set_frozen(true);
    control.begin_tick();
    assert!(!control.runs_normally());
    assert!(control.step(2));
    for _ in 0..2 {
        control.begin_tick();
        assert!(control.runs_normally());
    }
    control.begin_tick();
    assert!(!control.runs_normally(), "the step budget is spent");
    assert!(control.step(5));
    assert!(control.stop_stepping());
    assert!(!control.stop_stepping());
    control.begin_tick();
    assert!(!control.runs_normally());
}

#[test]
fn sprint_unfreezes_counts_down_and_restores_the_previous_freeze() {
    let mut control = TickControl::default();
    control.set_frozen(true);
    assert!(!control.request_sprint(3, REQUESTER));
    assert!(!control.frozen());
    assert!(control.sprinting());
    for _ in 0..3 {
        assert!(control.begin_tick().is_none());
        assert!(control.runs_normally());
        control.record_tick(2_000);
    }
    let report = control
        .begin_tick()
        .expect("sprint report after the last tick");
    assert_eq!(report.requester, REQUESTER);
    // 3 ticks in 6 ms: 500 ticks per second, 2.00 ms per tick.
    assert_eq!(report.ticks_per_second, 500);
    assert_eq!(report.millis_per_tick, "2.00");
    assert!(!control.sprinting());
    assert!(control.frozen(), "the pre-sprint freeze returns");
}

#[test]
fn stopping_a_sprint_reports_the_completed_part_and_a_second_request_interrupts() {
    let mut control = TickControl::default();
    assert!(control.stop_sprint().is_none());
    control.request_sprint(10, REQUESTER);
    for _ in 0..4 {
        control.begin_tick();
        control.record_tick(1_000);
    }
    assert!(control.request_sprint(20, REQUESTER), "restart interrupts");
    control.begin_tick();
    control.record_tick(1_000);
    let report = control.stop_sprint().expect("running sprint");
    assert_eq!(report.ticks_per_second, 1_000);
    assert!(control.stop_sprint().is_none());
}

#[test]
fn query_statistics_use_recorded_tick_times() {
    let mut control = TickControl::default();
    assert_eq!(control.percentiles().1, 0);
    for micros in (1..=100).map(|n| n * 1_000) {
        control.record_tick(micros);
    }
    assert!((control.average_millis() - 50.5).abs() < 1e-3);
    let ([p50, p95, p99], count) = control.percentiles();
    assert_eq!(count, 100);
    assert_eq!((p50, p95, p99), (51.0, 96.0, 100.0));
    // The ring keeps only the newest hundred samples.
    control.record_tick(500_000);
    assert_eq!(control.percentiles().1, 100);
    assert!(control.average_millis() > 50.5);
}

fn fixture() -> TickState {
    let registry = Arc::new(crate::registry::CoreRegistry::load(None).expect("registry"));
    let world = flat_world(&registry);
    TickState::new(registry, world, 2, 512)
}

fn join(state: &mut TickState) -> crate::boundary::EntityId {
    run_tick(
        state,
        &[PlayerInput::Join {
            connection: ConnectionId(1),
            uuid: uuid::Uuid::new_v4(),
            name: "steve".to_owned(),
            permission_level: 4,
        }],
    );
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    run_tick(
        state,
        &[PlayerInput::ConfirmTeleport {
            connection: ConnectionId(1),
            teleport_id: 1,
        }],
    );
    state
        .connections
        .entity_of(ConnectionId(1))
        .expect("joined")
}

fn command(state: &mut TickState, line: &str) -> Vec<PlayerChange> {
    let mut changes = ChangeSet::default();
    state.outbox.clear();
    crate::command_console::execute(state, ConnectionId(1), line, 4, &mut changes);
    state.outbox.remove(&ConnectionId(1)).unwrap_or_default()
}

fn feedback_keys(changes: &[PlayerChange]) -> Vec<String> {
    changes
        .iter()
        .filter_map(|change| match change {
            PlayerChange::StyledChat {
                content: mc_nbt::Tag::Compound(fields),
            } => fields.iter().find_map(|(key, value)| {
                (key == "translate").then(|| match value {
                    mc_nbt::Tag::String(text) => text.clone(),
                    _ => String::new(),
                })
            }),
            _ => None,
        })
        .collect()
}

#[test]
fn freeze_holds_mobs_time_and_weather_but_not_players_and_step_resumes_them() {
    let mut state = fixture();
    let player = join(&mut state);
    let slot = state.entities.slot_of(player).expect("player slot");
    let at = state.entities.pos(slot);
    // A zombie released above the floor falls only while the world runs.
    let zombie = state.registry.zombie_type_id;
    let mob = state.spawn_mob(zombie, [at[0] + 3.0, at[1] + 6.0, at[2]], 20.0);
    let mob_slot = state.entities.slot_of(mob).expect("mob slot");
    for _ in 0..3 {
        run_tick(&mut state, &[]);
    }
    let falling = state.entities.pos(mob_slot)[1];
    assert!(falling < at[1] + 6.0, "the control mob falls");

    let keys = feedback_keys(&command(&mut state, "tick freeze"));
    assert_eq!(keys, ["commands.tick.status.frozen"]);
    let (day, tick) = (state.day_time, state.tick);
    let held = state.entities.pos(mob_slot);
    for _ in 0..5 {
        run_tick(&mut state, &[]);
    }
    assert_eq!(state.entities.pos(mob_slot), held, "frozen mob stays put");
    assert_eq!(state.day_time, day, "frozen day clock");
    assert_eq!(state.tick, tick + 5, "WAL tick numbering keeps advancing");

    let keys = feedback_keys(&command(&mut state, "tick step 2"));
    assert_eq!(keys, ["commands.tick.step.success"]);
    run_tick(&mut state, &[]);
    let one = state.entities.pos(mob_slot);
    assert!(one[1] < held[1], "a stepped tick simulates");
    run_tick(&mut state, &[]);
    let two = state.entities.pos(mob_slot);
    assert!(two[1] < one[1]);
    run_tick(&mut state, &[]);
    assert_eq!(state.entities.pos(mob_slot), two, "steps are spent");

    let keys = feedback_keys(&command(&mut state, "tick unfreeze"));
    assert_eq!(keys, ["commands.tick.status.running"]);
    run_tick(&mut state, &[]);
    assert!(state.entities.pos(mob_slot)[1] <= two[1]);
    assert_eq!(
        state.day_time,
        day + 3,
        "two stepped ticks + one running tick"
    );
}

#[test]
fn commands_report_through_vanilla_keys_and_clients_learn_the_state() {
    let mut state = fixture();
    join(&mut state);

    let changes = command(&mut state, "tick rate 40");
    assert_eq!(feedback_keys(&changes), ["commands.tick.rate.success"]);
    assert!(changes.contains(&PlayerChange::TickingState {
        rate: 40.0,
        frozen: false
    }));
    assert_eq!(state.tick_control.period(), Duration::from_millis(25));

    let changes = command(&mut state, "tick step");
    assert_eq!(
        feedback_keys(&changes),
        ["commands.tick.step.fail"],
        "step needs a frozen game"
    );

    let changes = command(&mut state, "tick freeze");
    assert!(changes.contains(&PlayerChange::TickingState {
        rate: 40.0,
        frozen: true
    }));
    let changes = command(&mut state, "tick step 5t");
    assert!(changes.contains(&PlayerChange::TickingStep { steps: 5 }));
    let changes = command(&mut state, "tick step stop");
    assert_eq!(feedback_keys(&changes), ["commands.tick.step.stop.success"]);
    assert!(changes.contains(&PlayerChange::TickingStep { steps: 0 }));
    assert_eq!(
        feedback_keys(&command(&mut state, "tick step stop")),
        ["commands.tick.step.stop.fail"]
    );

    let changes = command(&mut state, "tick sprint 3");
    assert_eq!(feedback_keys(&changes), ["commands.tick.status.sprinting"]);
    assert!(state.tick_control.sprinting());
    assert!(!state.tick_control.frozen());
    let mut drained = Vec::new();
    for _ in 0..4 {
        let (_, drains, _) = run_tick(&mut state, &[]);
        for (connection, queue) in drains {
            if connection == ConnectionId(1) {
                drained.extend(queue);
            }
        }
    }
    assert!(
        !state.tick_control.sprinting(),
        "three ticks completed the sprint"
    );
    assert!(state.tick_control.frozen(), "pre-sprint freeze restored");
    assert!(
        feedback_keys(&drained).contains(&"commands.tick.sprint.report".to_owned()),
        "the requester gets the sprint report"
    );
    assert_eq!(
        feedback_keys(&command(&mut state, "tick sprint stop")),
        ["commands.tick.sprint.stop.fail"]
    );

    let changes = command(&mut state, "tick query");
    assert_eq!(
        feedback_keys(&changes),
        [
            "commands.tick.status.frozen",
            "commands.tick.query.rate.running",
            "commands.tick.query.percentiles"
        ]
    );
}

#[test]
fn joining_players_receive_the_current_ticking_state() {
    let mut state = fixture();
    state.tick_control.set_rate(5.0);
    state.tick_control.set_frozen(true);
    let (_, drains, _) = run_tick(
        &mut state,
        &[PlayerInput::Join {
            connection: ConnectionId(3),
            uuid: uuid::Uuid::new_v4(),
            name: "alex".to_owned(),
            permission_level: 0,
        }],
    );
    let queue = &drains
        .iter()
        .find(|(connection, _)| *connection == ConnectionId(3))
        .expect("join output")
        .1;
    assert!(queue.contains(&PlayerChange::TickingState {
        rate: 5.0,
        frozen: true
    }));
    assert!(queue.contains(&PlayerChange::TickingStep { steps: 0 }));
}

#[test]
fn parser_follows_vanilla_grammar_and_bounds() {
    use crate::commands::{Command, parse};
    let tick = |line: &str| match parse(line, true) {
        Ok(Command::Tick(command)) => Ok(command),
        Ok(other) => panic!("not a tick command: {other:?}"),
        Err(error) => Err(error.message()),
    };
    assert_eq!(tick("tick query"), Ok(TickCommand::Query));
    assert_eq!(tick("tick rate 20"), Ok(TickCommand::Rate(20.0)));
    assert_eq!(tick("tick rate 10000"), Ok(TickCommand::Rate(10_000.0)));
    assert!(
        tick("tick rate 0.5")
            .unwrap_err()
            .contains("must not be less than 1.0")
    );
    assert!(
        tick("tick rate 10001")
            .unwrap_err()
            .contains("must not be more than 10000.0")
    );
    assert_eq!(tick("tick step"), Ok(TickCommand::Step(1)));
    assert_eq!(tick("tick step stop"), Ok(TickCommand::StepStop));
    assert_eq!(tick("tick step 1s"), Ok(TickCommand::Step(20)));
    assert_eq!(tick("tick step 2d"), Ok(TickCommand::Step(48_000)));
    assert!(
        tick("tick step 0")
            .unwrap_err()
            .contains("Tick count must not be less than 1")
    );
    assert_eq!(tick("tick sprint 60s"), Ok(TickCommand::Sprint(1_200)));
    assert_eq!(tick("tick sprint stop"), Ok(TickCommand::SprintStop));
    assert_eq!(tick("tick freeze"), Ok(TickCommand::Freeze));
    assert_eq!(tick("tick unfreeze"), Ok(TickCommand::Unfreeze));
    assert!(tick("tick").is_err());
    assert!(tick("tick sprint").is_err());
    assert!(parse("tick query", false).is_err(), "operators only");
}

#[test]
fn game_time_stands_still_while_frozen_and_counts_stepped_ticks() {
    let mut control = TickControl::default();
    control.begin_tick();
    assert_eq!(control.game_ticks(1), 1);
    control.set_frozen(true);
    for tick in 2..=4 {
        control.begin_tick();
        assert_eq!(control.game_ticks(tick), 1, "frozen ticks add no game time");
    }
    control.step(1);
    control.begin_tick();
    assert_eq!(control.game_ticks(5), 2, "the stepped tick counts");
    control.begin_tick();
    assert_eq!(control.game_ticks(6), 2);
}
