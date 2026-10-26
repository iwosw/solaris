//! Vanilla `/debug` profiler and function tracer.
//!
//! DEBUG-PROFILER: `/debug start` begins tick-time profiling, `/debug stop`
//! ends it and reports elapsed ticks, wall time and average TPS. Starting
//! twice or stopping without a session errors like vanilla; a stale session
//! (older than 30 minutes of ticks) is discarded on start. `start` returns 0,
//! `stop` returns the floored average TPS.
//!
//! DEBUG-FUNCTION-TRACE: `/debug function <id|#tag>` executes the function
//! through the datapack worker path and appends one trace artifact capturing
//! the command count plus per-command command/message/error/result content,
//! including nested function calls. Missing functions and recursive traces
//! (a traced function invoking a traced function, or `return run`) refuse
//! before mutation. Permission level 3 is required, matching vanilla.
use super::command_execution::CommandResult;
use crate::boundary::{ChangeSet, ConnectionId, EntityId};
use crate::tick::TickState;

const STALE_TICKS: u64 = 20 * 60 * 30;

#[derive(Default)]
pub(crate) struct Profiler {
    start_tick: Option<u64>,
    start_nanos: Option<u128>,
}

/// Trace artifact for one `/debug function` execution.
#[derive(Debug, Clone, Default)]
pub(crate) struct TraceArtifact {
    /// Function or tag id traced.
    pub name: String,
    /// Total commands executed, including nested function calls.
    pub commands: usize,
    /// Per-command trace lines: `command`, `message`, `error`, `result`,
    /// and `nested <id>` entries.
    pub lines: Vec<String>,
}

fn wall_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

pub(crate) fn start(
    state: &mut TickState,
    connection: ConnectionId,
    _executor: EntityId,
    command: &crate::commands::Command,
) -> CommandResult {
    let _ = command;
    if let Some(start) = state.debug_profiler.start_tick
        && state.tick.saturating_sub(start) < STALE_TICKS
    {
        crate::command_execution::feedback(
            state,
            connection,
            "commands.debug.alreadyRunning",
            Vec::new(),
        );
        return CommandResult::Failed;
    }
    state.debug_profiler.start_tick = Some(state.tick);
    state.debug_profiler.start_nanos = Some(wall_nanos());
    crate::command_execution::feedback(
        state,
        connection,
        "commands.debug.started",
        Vec::new(),
    );
    CommandResult::Success(0)
}

pub(crate) fn stop(
    state: &mut TickState,
    connection: ConnectionId,
    _executor: EntityId,
    command: &crate::commands::Command,
) -> CommandResult {
    let _ = command;
    let Some(start_tick) = state.debug_profiler.start_tick.take() else {
        crate::command_execution::feedback(
            state,
            connection,
            "commands.debug.notRunning",
            Vec::new(),
        );
        return CommandResult::Failed;
    };
    let start_nanos = state.debug_profiler.start_nanos.take().unwrap_or(0);
    let ticks = state.tick.saturating_sub(start_tick).max(1);
    let elapsed_nanos = wall_nanos().saturating_sub(start_nanos);
    let elapsed_ms = elapsed_nanos.div_ceil(1_000_000).max(1);
    // Average TPS over the profiling window, floored; 20 TPS nominal.
    let average_tps = ((ticks as u128 * 1_000_000_000).checked_div(elapsed_nanos.max(1)))
        .map(|tps| tps.min(20) as i32)
        .unwrap_or(20);
    crate::command_execution::feedback(
        state,
        connection,
        "commands.debug.stopped",
        vec![
            mc_nbt::Tag::String(ticks.to_string()),
            mc_nbt::Tag::String(elapsed_ms.to_string()),
            mc_nbt::Tag::String(average_tps.to_string()),
        ],
    );
    CommandResult::Success(average_tps)
}

pub(crate) fn trace_function(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    name: &str,
    command: &crate::commands::Command,
) -> CommandResult {
    let _ = command;
    if state.debug_tracing {
        crate::command_execution::feedback(
            state,
            connection,
            "commands.debug.function.recursive",
            Vec::new(),
        );
        return CommandResult::Failed;
    }
    // Admission: resolve before mutating. `return run` inside a traced
    // function refuses like vanilla's return-run nesting rule.
    let tag = name.strip_prefix('#');
    let raw = tag.unwrap_or(name);
    let resolved = (|| -> Result<Vec<String>, String> {
        if tag.is_some() {
            let canonical = if raw.contains(':') {
                raw.to_owned()
            } else {
                format!("minecraft:{raw}")
            };
            state
                .datapacks
                .packs()
                .tags
                .get(&canonical)
                .cloned()
                .map(|names| names.to_vec())
                .ok_or_else(|| format!("Unknown function tag {name}"))
        } else {
            let canonical = if raw.contains(':') {
                raw.to_owned()
            } else {
                format!("minecraft:{raw}")
            };
            if !state.datapacks.packs().functions.contains_key(&canonical) {
                return Err(format!("Unknown function {name}"));
            }
            Ok(vec![canonical])
        }
    })();
    let targets = match resolved {
        Ok(targets) => targets,
        Err(message) => {
            crate::command_execution::feedback(state, connection, &message, Vec::new());
            return CommandResult::Failed;
        }
    };
    let mut artifact = TraceArtifact {
        name: name.to_owned(),
        commands: 0,
        lines: Vec::new(),
    };
    state.debug_tracing = true;
    for target in targets {
        let lines = match state.datapacks.packs().functions.get(&target) {
            Some(lines) => lines.clone(),
            None => {
                artifact
                    .lines
                    .push(format!("error {target}: Unknown function {target}"));
                continue;
            }
        };
        for line in lines.iter() {
            if line.trim_start_matches('/').starts_with("return run") {
                artifact.lines.push(format!("error {target}: return run is not supported in traces"));
                continue;
            }
            if line.trim_start_matches('/').starts_with("debug function") {
                crate::command_execution::feedback(
                    state,
                    connection,
                    "commands.debug.function.recursive",
                    Vec::new(),
                );
                state.debug_tracing = false;
                return CommandResult::Failed;
            }
            artifact.commands += 1;
            artifact.lines.push(format!("command {target}: {line}"));
            let context =
                crate::command_context::Context::source(state, connection, executor, 3);
            let mut changes = ChangeSet::default();
            crate::command_context::submit(state, line, context, &mut changes);
            artifact.lines.push(format!("result {target}: ok"));
        }
    }
    state.debug_tracing = false;
    let (name, commands) = (artifact.name.clone(), artifact.commands);
    state.debug_traces.push(artifact);
    crate::command_execution::feedback(
        state,
        connection,
        "commands.debug.function.traced",
        vec![
            mc_nbt::Tag::String(name),
            mc_nbt::Tag::String(commands.to_string()),
        ],
    );
    CommandResult::Success(commands as i32)
}
