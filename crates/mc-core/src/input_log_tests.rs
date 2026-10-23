//! Input-log tests: format, credential redaction, the live-loop wiring, and
//! the test-only replayer (checkpoint copy + logged inputs through `run_tick`
//! must reproduce the original run's canonical state).

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::boundary::{BlockPos, ConnectionId, DigAction, EntityId, Hand, PlayerInput};
use crate::config::CoreConfig;
use crate::core::{Core, apply_spawn_request, apply_ticket, recover_tick_state};
use crate::entities::EntitySlot;
use crate::input_log::{
    AUTH_COMMAND, InputLog, LOCKED_SESSION_TEXT, Line, LoggedInput, LoggedSpawn, LoggedTicket,
    TickRecord,
};
use crate::tick::{TickState, run_tick};

const PASSWORD: &str = "hunter2-test-password";

fn temp_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mc-core-input-log-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

pub(crate) fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

// ------------------------------------------------------------------ reader

struct ParsedLog {
    start_tick: u64,
    uncovered: Vec<String>,
    ticks: BTreeMap<u64, TickRecord>,
    end_tick: Option<u64>,
    gaps: Vec<(u64, u64)>,
}

fn log_file(world: &Path) -> PathBuf {
    let mut files: Vec<PathBuf> = std::fs::read_dir(world.join(crate::input_log::DIR))
        .expect("input-log dir")
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "one log file per Core::open: {files:?}");
    files.pop().unwrap()
}

fn read_log(path: &Path) -> ParsedLog {
    let text = std::fs::read_to_string(path).unwrap();
    let mut lines = text.lines().map(|line| {
        serde_json::from_str::<Line>(line).unwrap_or_else(|e| panic!("bad line {line}: {e}"))
    });
    let Some(Line::Header {
        format,
        start_tick,
        uncovered,
    }) = lines.next()
    else {
        panic!("first line is the header");
    };
    assert_eq!(format, crate::input_log::FORMAT);
    let mut log = ParsedLog {
        start_tick,
        uncovered,
        ticks: BTreeMap::new(),
        end_tick: None,
        gaps: Vec::new(),
    };
    for line in lines {
        match line {
            Line::Tick(record) => {
                assert!(log.end_tick.is_none(), "nothing after the end marker");
                assert!(log.ticks.insert(record.tick, record).is_none());
            }
            Line::End { tick } => log.end_tick = Some(tick),
            Line::Gap { from, to } => log.gaps.push((from, to)),
            Line::Header { .. } => panic!("second header"),
        }
    }
    log
}

// ---------------------------------------------------------------- replayer

/// Feed the logged records back, tick by tick, through the same `run_tick`
/// the live loop uses. Chunk residency is pinned (blocking) to the demand
/// tick, exactly as the original driver in these tests does.
fn replay(state: &mut TickState, log: &ParsedLog) -> Result<(), String> {
    if let Some((from, to)) = log.gaps.first() {
        return Err(format!("log has a gap: ticks {from}..={to} were dropped"));
    }
    if state.tick != log.start_tick {
        return Err(format!(
            "world is at tick {}, log starts at {}",
            state.tick, log.start_tick
        ));
    }
    let end = log
        .end_tick
        .or_else(|| log.ticks.keys().next_back().copied())
        .unwrap_or(log.start_tick);
    for tick in log.start_tick + 1..=end {
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        let mut inputs = Vec::new();
        if let Some(record) = log.ticks.get(&tick) {
            for input in &record.inputs {
                match input {
                    LoggedInput::Input(input) => inputs.push(input.clone()),
                    // Replayed sessions are already authenticated (no auth gate).
                    LoggedInput::Uncovered { kind, .. }
                        if kind == AUTH_COMMAND || kind == LOCKED_SESSION_TEXT => {}
                    LoggedInput::Uncovered { kind, connection } => {
                        return Err(format!(
                            "tick {tick}: uncovered `{kind}` input from {connection:?}"
                        ));
                    }
                }
            }
            for spawn in &record.spawns {
                apply_spawn_request(state, &spawn.request());
            }
            for ticket in &record.tickets {
                apply_ticket(
                    state,
                    ticket.owner.clone(),
                    ticket.x,
                    ticket.z,
                    ticket.radius,
                );
            }
        }
        run_tick(state, &inputs);
        if state.tick != tick {
            return Err(format!("tick {tick} ran as {}", state.tick));
        }
    }
    Ok(())
}

pub(crate) fn open_world_state(config: &CoreConfig) -> TickState {
    let mut registry =
        crate::registry::CoreRegistry::load(config.vanilla_data_dir.as_deref()).expect("registry");
    crate::plugin_startup::apply(
        &mut registry,
        &mc_plugin_host::DeploymentContribution::default(),
        None,
        &[],
    )
    .expect("plugin startup");
    let registry = Arc::new(registry);
    let path = config.world_dir.join("native/state.redb");
    let store = Arc::new(
        mc_world::native::NativeStore::open(
            &path,
            crate::native_checkpoint::native_identity(config),
        )
        .expect("open native store"),
    );
    let mut state = recover_tick_state(config, &registry, &store, false, None, None)
        .expect("recover")
        .0;
    // The one wall-clock read the tick makes (advancement timestamps) is
    // pinned, as in `replay_determinism_tests`; a production replay would need
    // it logged (see the report).
    state.wall_clock = || 1_767_225_600;
    state
}

// -------------------------------------------------------------------- hash

fn h<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

/// Canonical state, one hash per component. Nothing is masked: entity uuids,
/// loader-ticket counters and the wall clock are pinned by the engine.
fn canonical(state: &TickState) -> BTreeMap<&'static str, u64> {
    let mut out = BTreeMap::new();
    let mut resident: Vec<(i32, i32)> = state
        .world
        .resident_positions()
        .iter()
        .map(|pos| (pos.x, pos.z))
        .collect();
    resident.sort_unstable();
    out.insert("residency", h(&resident));
    let mut blocks = std::collections::hash_map::DefaultHasher::new();
    for &(x, z) in &resident {
        let chunk = state
            .world
            .chunk(crate::boundary::ChunkPos { x, z })
            .expect("resident");
        for (index, section) in chunk.sections.iter().enumerate() {
            (x, z, index, section.non_air_count()).hash(&mut blocks);
            for y in 0..16u8 {
                for zz in 0..16u8 {
                    for xx in 0..16u8 {
                        section.get(xx, y, zz).0.hash(&mut blocks);
                    }
                }
            }
        }
    }
    out.insert("blocks", blocks.finish());
    let mut entities: BTreeMap<i32, u64> = BTreeMap::new();
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        entities.insert(
            body.id.0,
            h(&format!(
                "{body:?}|{:?}|{:?}|{:?}|{:?}",
                state.entities.row(slot),
                state.entities.pos(slot),
                state.entities.vel(slot),
                state.entities.rot(slot)
            )),
        );
    }
    out.insert("entities", h(&entities));
    let mut ids: Vec<EntityId> = state.players.keys().copied().collect();
    ids.sort();
    let players: Vec<u64> = ids
        .iter()
        .map(|id| {
            let p = &state.players[id];
            h(&format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{}",
                p.inventory,
                p.vitals,
                p.game_mode,
                p.active_effects,
                p.enchantment_seed,
                p.fall_distance,
                p.sprinting,
                p.advancement_progress
            ))
        })
        .collect();
    out.insert("players", h(&players));
    out.insert("entity_count", state.entities.len() as u64);
    out
}

// ------------------------------------------------------------------ script

fn cmd(connection: ConnectionId, command: &str) -> PlayerInput {
    PlayerInput::Command {
        connection,
        command: command.to_owned(),
        permission_level: 4,
    }
}

/// Scripted client: timed commands are open-loop; teleport confirmation,
/// walking and block-edit targets read the server state like a real client.
fn script(i: u64, state: &TickState) -> Vec<PlayerInput> {
    let c1 = ConnectionId(1);
    let mut out = Vec::new();
    let player = state.connections.entity_of(c1);
    if let Some(teleport_id) = player
        .and_then(|id| state.players.get(&id))
        .and_then(|p| p.awaiting_teleport)
    {
        out.push(PlayerInput::ConfirmTeleport {
            connection: c1,
            teleport_id,
        });
    }
    match i {
        0 => out.push(PlayerInput::Join {
            connection: c1,
            uuid: uuid::Uuid::from_u128(0xA570_0001),
            name: "replay1".to_owned(),
            permission_level: 4,
        }),
        2 => {
            out.push(cmd(c1, "gamemode creative"));
            out.push(cmd(c1, "give @s minecraft:stone 64"));
            out.push(cmd(c1, &format!("login {PASSWORD}")));
        }
        3 => {
            out.push(cmd(c1, "summon minecraft:cow ~4 ~ ~3"));
            out.push(cmd(c1, "summon minecraft:zombie ~-5 ~ ~"));
            out.push(cmd(c1, "summon minecraft:pig ~3 ~ ~-4"));
        }
        20 => out.push(cmd(c1, "setblock ~3 ~5 ~ minecraft:sand")),
        25 => out.push(cmd(c1, "setblock ~4 ~2 ~1 minecraft:water")),
        40 => out.push(cmd(c1, "fill ~4 ~ ~4 ~9 ~3 ~9 minecraft:stone")),
        45 => out.push(PlayerInput::DropSelected {
            connection: c1,
            whole_stack: false,
        }),
        _ => {}
    }
    let pos = player
        .and_then(|id| state.entities.slot_of(id))
        .map(|slot| (state.entities.pos(slot), state.entities.ground(slot)));
    if i >= 5
        && let Some((pos, on_ground)) = pos
    {
        out.push(PlayerInput::Move {
            connection: c1,
            position: [pos[0] + 0.2, pos[1], pos[2]],
            yaw: -90.0,
            pitch: 0.0,
            on_ground,
        });
    }
    if i >= 8
        && i.is_multiple_of(7)
        && let Some((pos, _)) = pos
    {
        let k = (i / 7) as i32;
        let (bx, bz) = (
            pos[0].floor() as i32 + 2 + k % 5,
            pos[2].floor() as i32 + k % 3,
        );
        let air = state.registry.states.air;
        let by = (-60..320)
            .rev()
            .find(|&y| state.world.block_at(bx, y, bz).is_some_and(|s| s != air))
            .unwrap_or(60);
        let position = BlockPos {
            x: bx,
            y: by,
            z: bz,
        };
        out.push(if k % 2 == 0 {
            PlayerInput::Dig {
                connection: c1,
                position,
                action: DigAction::Start,
                sequence: i as i32,
            }
        } else {
            PlayerInput::UseItemOn {
                connection: c1,
                hand: Hand::Main,
                position,
                face: 1,
                cursor: [0.5; 3],
                sequence: i as i32,
            }
        });
    }
    out
}

// ------------------------------------------------------------------- tests

/// The headline: copy a checkpointed world, run the original session with the
/// log on, then replay the log from the copy. Same canonical state.
#[test]
fn replay_from_checkpoint_copy_reproduces_original_state() {
    const TICKS: u64 = 120;
    let original_dir = temp_dir("original");
    let replay_dir = temp_dir("replay");
    let config = CoreConfig::new(original_dir.clone(), 42);
    // Fresh world: the clean shutdown leaves its checkpoint.
    let core = Core::open(config.clone()).expect("create world");
    core.shutdown();
    drop(core);
    let gapped_dir = temp_dir("gapped");
    copy_dir(&original_dir, &replay_dir);
    copy_dir(&original_dir, &gapped_dir);

    // Original run: the same drain -> record -> run_tick order as `tick_loop`.
    let mut state = open_world_state(&config);
    let before = canonical(&state);
    let start = state.tick;
    let mut log = InputLog::open(&original_dir, start, Vec::new()).expect("open log");
    for i in 0..TICKS {
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        let inputs = script(i, &state);
        let (output, ..) = run_tick(&mut state, &inputs);
        log.record(output.tick, inputs, Vec::new(), Vec::new(), Vec::new());
    }
    log.finish(state.tick);
    let original = canonical(&state);
    assert_ne!(original, before, "the script changed the world");
    assert!(original["entity_count"] > 1, "player plus spawned mobs");
    drop(state);

    let parsed = read_log(&log_file(&original_dir));
    assert_eq!(parsed.start_tick, start);
    assert_eq!(parsed.end_tick, Some(start + TICKS));
    assert!(parsed.ticks.len() > 20, "{} records", parsed.ticks.len());

    let replay_config = CoreConfig::new(replay_dir.clone(), 42);
    let mut copy = open_world_state(&replay_config);
    replay(&mut copy, &parsed).expect("replay");
    assert_eq!(canonical(&copy), original);

    // Negative control: without the `/fill` tick the replay must differ.
    let mut gapped = read_log(&log_file(&original_dir));
    assert!(
        gapped.ticks.remove(&(start + 41)).is_some(),
        "fill tick logged"
    );
    let mut other = open_world_state(&CoreConfig::new(gapped_dir.clone(), 42));
    replay(&mut other, &gapped).expect("replay without the fill");
    assert_ne!(canonical(&other), original);
    let _ = std::fs::remove_dir_all(gapped_dir);

    let _ = std::fs::remove_dir_all(original_dir);
    let _ = std::fs::remove_dir_all(replay_dir);
}

/// Credentials never reach the file: the auth command is a marker, and the
/// replayer treats the session as already authenticated.
#[test]
fn auth_command_text_never_reaches_the_log() {
    let dir = temp_dir("auth");
    let mut log = InputLog::open(&dir, 0, Vec::new()).unwrap();
    let c = ConnectionId(3);
    log.record(
        1,
        vec![
            cmd(c, &format!("login {PASSWORD}")),
            cmd(c, &format!("/Register {PASSWORD} {PASSWORD}")),
            cmd(c, &format!("  changepassword {PASSWORD} new{PASSWORD}")),
            cmd(c, "gamemode creative"),
        ],
        Vec::new(),
        Vec::new(),
        Vec::new(),
    );
    log.finish(1);
    let path = log_file(&dir);
    let raw = std::fs::read_to_string(&path).unwrap();
    assert!(!raw.contains(PASSWORD), "password leaked into the log");
    let parsed = read_log(&path);
    let kinds: Vec<_> = parsed.ticks[&1]
        .inputs
        .iter()
        .map(|input| match input {
            LoggedInput::Uncovered { kind, .. } => kind.as_str(),
            LoggedInput::Input(_) => "input",
        })
        .collect();
    assert_eq!(kinds, [AUTH_COMMAND, AUTH_COMMAND, AUTH_COMMAND, "input"]);
    let _ = std::fs::remove_dir_all(dir);
}

/// Representative inputs survive the codec; a non-finite float (which JSON
/// cannot carry) becomes a fail-closed marker instead of an unreadable line.
#[test]
fn codec_round_trips_and_marks_unserializable_ticks() {
    use mc_nbt::Tag;
    let c = ConnectionId(1);
    let (console, _waiter) = crate::command_console::ConsoleCommand::new("say hi".to_owned());
    let samples = vec![
        PlayerInput::Move {
            connection: c,
            position: [1.5, -64.0, 1e-7],
            yaw: 33.333_332,
            pitch: -0.1,
            on_ground: true,
        },
        PlayerInput::DialogClick {
            connection: c,
            id: "solaris:test".to_owned(),
            payload: Some(Tag::Compound(vec![("k".to_owned(), Tag::Int(7))])),
        },
        PlayerInput::DialogClick {
            connection: c,
            id: "solaris:none".to_owned(),
            payload: None,
        },
        PlayerInput::LoaderViewRequest {
            connection: c,
            owner: "plugin".to_owned(),
            request_kind: mc_script::ScriptClientViewKey::try_new("solaris:view").unwrap(),
        },
        PlayerInput::CreativeSlot {
            connection: c,
            slot: 36,
            stack: mc_data::ItemStack::default(),
        },
        PlayerInput::ConsoleCommand(console),
    ];
    for input in samples {
        let line = serde_json::to_string(&LoggedInput::Input(input)).unwrap();
        let back: LoggedInput = serde_json::from_str(&line).unwrap();
        assert_eq!(serde_json::to_string(&back).unwrap(), line);
    }

    let config = CoreConfig::new(temp_dir("nan-world"), 42);
    let core = Core::open(config.clone()).unwrap();
    core.shutdown();
    drop(core);
    let mut state = open_world_state(&config);
    let (start, tick) = (state.tick, state.tick + 1);
    let dir = temp_dir("nan");
    let mut log = InputLog::open(&dir, start, vec!["plugin_runtime".to_owned()]).unwrap();
    log.record(
        tick,
        vec![PlayerInput::Move {
            connection: c,
            position: [f64::NAN, 0.0, 0.0],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: true,
        }],
        vec![LoggedSpawn {
            type_id: 1,
            count: 2,
            spread: true,
            start: 0,
            total: 2,
        }],
        vec![LoggedTicket {
            owner: "t".to_owned(),
            x: -1,
            z: 2,
            radius: 3,
        }],
        Vec::new(),
    );
    log.finish(tick);
    let parsed = read_log(&log_file(&dir));
    assert_eq!(parsed.uncovered, ["plugin_runtime"]);
    let record = &parsed.ticks[&tick];
    assert_eq!((record.spawns.len(), record.tickets.len()), (1, 1));
    assert!(matches!(
        record.inputs.as_slice(),
        [LoggedInput::Uncovered { kind, .. }] if kind == crate::input_log::UNSERIALIZABLE
    ));
    // Replay refuses a tick it cannot reproduce.
    assert!(
        replay(&mut state, &parsed)
            .unwrap_err()
            .contains(crate::input_log::UNSERIALIZABLE)
    );
    let _ = std::fs::remove_dir_all(dir);
    let _ = std::fs::remove_dir_all(config.world_dir);
}

/// Text from a locked (unauthenticated) session is refused by the auth gate
/// anyway; it must never be written, whatever its kind. Other connections'
/// text and non-text inputs of the locked one are untouched.
#[test]
fn locked_session_text_is_withheld() {
    let dir = temp_dir("locked");
    let mut log = InputLog::open(&dir, 0, Vec::new()).unwrap();
    let (locked, free) = (ConnectionId(5), ConnectionId(6));
    let chat = |connection, message: &str| PlayerInput::Chat {
        connection,
        message: message.to_owned(),
    };
    log.record(
        1,
        vec![
            chat(locked, PASSWORD),
            PlayerInput::RenameItem {
                connection: locked,
                name: PASSWORD.to_owned(),
            },
            PlayerInput::SignUpdate {
                connection: locked,
                position: BlockPos { x: 0, y: 0, z: 0 },
                front: true,
                lines: vec![PASSWORD.to_owned()],
            },
            PlayerInput::DialogClick {
                connection: locked,
                id: "solaris:login".to_owned(),
                payload: Some(mc_nbt::Tag::String(PASSWORD.to_owned())),
            },
            cmd(locked, &format!("msg x {PASSWORD}")),
            PlayerInput::MoveStatus {
                connection: locked,
                on_ground: true,
            },
            chat(free, "hello"),
        ],
        Vec::new(),
        Vec::new(),
        vec![locked],
    );
    log.finish(1);
    let path = log_file(&dir);
    assert!(!std::fs::read_to_string(&path).unwrap().contains(PASSWORD));
    let parsed = read_log(&path);
    let kinds: Vec<_> = parsed.ticks[&1]
        .inputs
        .iter()
        .map(|input| match input {
            LoggedInput::Uncovered { kind, .. } => kind.as_str(),
            LoggedInput::Input(_) => "input",
        })
        .collect();
    let withheld = LOCKED_SESSION_TEXT;
    assert_eq!(
        kinds,
        [
            withheld, withheld, withheld, withheld, withheld, "input", "input"
        ]
    );
    let _ = std::fs::remove_dir_all(dir);
}

/// Through the real loop with `require_plugin_login`: a password typed into
/// chat before and after the join was processed never reaches the file.
#[test]
fn live_locked_chat_never_reaches_the_log() {
    let dir = temp_dir("live-locked");
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    config.input_log = true;
    config.require_plugin_login = true;
    let core = Core::open(config).expect("open");
    let mut rx = core.subscribe();
    let connection = ConnectionId(21);
    let chat = || PlayerInput::Chat {
        connection,
        message: PASSWORD.to_owned(),
    };
    assert!(core.submit(PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(0xCAFE),
        name: "locked".to_owned(),
        permission_level: 0,
    }));
    assert!(core.submit(chat()));
    let mut seen = rx.blocking_recv().expect("tick thread alive").tick;
    assert!(core.session_locked(connection));
    assert!(core.submit(chat()));
    let target = seen + 3;
    while seen < target {
        seen = rx.blocking_recv().expect("tick thread alive").tick;
    }
    core.shutdown();
    drop(core);
    let path = log_file(&dir);
    assert!(!std::fs::read_to_string(&path).unwrap().contains(PASSWORD));
    let markers = read_log(&path)
        .ticks
        .values()
        .flat_map(|record| &record.inputs)
        .filter(|input| {
            matches!(input, LoggedInput::Uncovered { kind, .. } if kind == LOCKED_SESSION_TEXT)
        })
        .count();
    assert_eq!(markers, 2, "both chat lines became markers");
    let _ = std::fs::remove_dir_all(dir);
}

/// A full writer queue never blocks the tick: records are dropped and one
/// gap marker names the missing ticks, in order; replay refuses the log.
#[test]
fn full_queue_drops_records_and_reports_a_gap() {
    let (mut log, rx) = InputLog::detached(1);
    let input = || vec![cmd(ConnectionId(1), "time query daytime")];
    for tick in 1..=3 {
        log.record(tick, input(), Vec::new(), Vec::new(), Vec::new());
    }
    let next =
        |rx: &std::sync::mpsc::Receiver<crate::input_log::Msg>| rx.try_recv().unwrap().describe();
    assert_eq!(next(&rx), "tick 1");
    log.record(4, input(), Vec::new(), Vec::new(), Vec::new());
    assert_eq!(next(&rx), "gap 2..=3", "gap precedes later records");
    log.record(5, input(), Vec::new(), Vec::new(), Vec::new());
    assert_eq!(
        next(&rx),
        "gap 4..=4",
        "each drop is reported before the log resumes"
    );
    drop(log);

    let gapped = ParsedLog {
        start_tick: 0,
        uncovered: Vec::new(),
        ticks: BTreeMap::new(),
        end_tick: Some(5),
        gaps: vec![(2, 3)],
    };
    let config = CoreConfig::new(temp_dir("gap-world"), 42);
    let core = Core::open(config.clone()).unwrap();
    core.shutdown();
    drop(core);
    let mut state = open_world_state(&config);
    assert!(replay(&mut state, &gapped).unwrap_err().contains("gap"));
    let _ = std::fs::remove_dir_all(config.world_dir);
}

/// The wiring in the real tick loop: what was submitted shows up at the tick
/// that consumed it, the end marker closes the file, and the default is off.
#[test]
fn live_core_logs_what_each_tick_consumed() {
    let dir = temp_dir("live");
    let mut config = CoreConfig::new(dir.clone(), 42);
    config.checkpoint_every_ticks = u64::MAX;
    config.input_log = true;
    let core = Core::open(config).expect("open");
    let mut rx = core.subscribe();
    let connection = ConnectionId(9);
    let join = PlayerInput::Join {
        connection,
        uuid: uuid::Uuid::from_u128(0xBEEF),
        name: "live".to_owned(),
        permission_level: 0,
    };
    assert!(core.submit(join.clone()));
    assert!(core.submit(cmd(connection, &format!("login {PASSWORD}"))));
    let mut last = 0;
    while last < 5 {
        last = rx.blocking_recv().expect("tick thread alive").tick;
    }
    core.shutdown();
    drop(core);

    let raw_path = log_file(&dir);
    assert!(
        !std::fs::read_to_string(&raw_path)
            .unwrap()
            .contains(PASSWORD)
    );
    let parsed = read_log(&raw_path);
    assert_eq!(parsed.start_tick, 0);
    assert!(parsed.uncovered.is_empty());
    assert!(parsed.end_tick.is_some_and(|end| end >= last));
    let joins: Vec<u64> = parsed
        .ticks
        .values()
        .filter(|record| {
            record
                .inputs
                .iter()
                .any(|input| matches!(input, LoggedInput::Input(i) if *i == join))
        })
        .map(|record| record.tick)
        .collect();
    assert_eq!(joins.len(), 1, "the join is consumed by exactly one tick");
    assert!(joins[0] >= 1 && joins[0] <= last);
    assert!(parsed.ticks.values().any(|record| record.inputs.iter().any(
        |input| matches!(input, LoggedInput::Uncovered { kind, .. } if kind == AUTH_COMMAND)
    )));
    let _ = std::fs::remove_dir_all(&dir);

    // Default off: no directory.
    let off = temp_dir("off");
    let core = Core::open(CoreConfig::new(off.clone(), 42)).expect("open");
    let mut rx = core.subscribe();
    rx.blocking_recv().expect("a tick");
    core.shutdown();
    drop(core);
    assert!(!off.join(crate::input_log::DIR).exists());
    let _ = std::fs::remove_dir_all(off);
}
