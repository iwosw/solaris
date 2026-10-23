//! Opt-in per-tick input log (`input_log = true`).
//!
//! The tick owner appends, per executed tick, exactly what that tick consumed
//! from outside the simulation: the drained [`PlayerInput`]s (including the
//! synthetic `Leave`s and the carried-over pending input), debug spawn
//! requests and remote chunk tickets. A session can later be replayed from the
//! checkpoint it started at by feeding these records back through `run_tick`
//! (`input_log_tests.rs` holds the test-only replayer).
//!
//! Format: one JSON document per line in
//! `<world>/input-log/<start_tick>-<unix_ms>.jsonl`, one file per `Core::open`.
//! Line 1 is [`Line::Header`]; then one [`Line::Tick`] per tick that consumed
//! anything (empty ticks are implicit); a clean stop appends [`Line::End`].
//!
//! The writer is a dedicated thread behind a bounded queue: the tick path
//! only moves the already-drained vectors and never blocks. If the queue is
//! full (stalled disk) the tick's record is dropped and one [`Line::Gap`]
//! marker later says which ticks are missing. Lines are flushed whenever the
//! queue runs dry and never fsynced, so a crash loses at most the unflushed
//! tail. Write failures are logged once and stop the log; they never touch
//! the tick.
//!
//! Secrets: auth commands (`login`/`register`/`changepassword`) and every
//! text-bearing input (chat, command, sign, rename, dialog click) of a
//! session the auth gate holds locked are replaced by markers before
//! serialization. Text typed into an *unlocked* session (chat, plugin
//! commands under other names) is logged as sent: the core cannot know it is
//! a secret.

use std::io::{BufWriter, Write};
use std::path::Path;
use std::sync::mpsc::{Receiver, SyncSender, TryRecvError, TrySendError, sync_channel};

use serde::{Deserialize, Serialize};

use crate::boundary::{ConnectionId, PlayerInput};
use crate::config::CoreError;
use crate::core::SpawnRequest;

/// Current line format version.
pub(crate) const FORMAT: u32 = 1;
/// World subdirectory holding the logs.
pub(crate) const DIR: &str = "input-log";
/// Marker kind for a credential-bearing command whose text was withheld.
pub(crate) const AUTH_COMMAND: &str = "auth_command";
/// Marker kind for text from a session the auth gate holds locked (refused by
/// the tick anyway, so a replay that skips it is equivalent).
pub(crate) const LOCKED_SESSION_TEXT: &str = "locked_session_text";
/// Marker kind for a frozen before-build plan that cannot be serialized.
pub(crate) const AUTHORIZED_BUILD: &str = "authorized_build";
/// Marker kind for a tick whose inputs did not survive a serialization round trip.
pub(crate) const UNSERIALIZABLE: &str = "unserializable_input";

/// One log line.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum Line {
    Header {
        format: u32,
        /// `TickState::tick` when logging began; the first record is at least `start_tick + 1`.
        start_tick: u64,
        /// Consumed-input classes this log does not contain (see module docs).
        uncovered: Vec<String>,
    },
    Tick(TickRecord),
    /// Clean stop: the last tick the owner ran.
    End {
        tick: u64,
    },
    /// Records of ticks `from..=to` were dropped (writer queue full): the log
    /// is not replayable across them.
    Gap {
        from: u64,
        to: u64,
    },
}

/// Everything tick `tick` consumed, in the order the loop applied it:
/// spawns, then tickets, then `run_tick(inputs)`.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct TickRecord {
    /// `TickOutput::tick` of the tick that consumed this record.
    pub tick: u64,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub spawns: Vec<LoggedSpawn>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tickets: Vec<LoggedTicket>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inputs: Vec<LoggedInput>,
}

#[derive(Debug, Serialize, Deserialize)]
pub(crate) enum LoggedInput {
    Input(PlayerInput),
    /// An input that was consumed but is not reproducible from the log.
    Uncovered {
        kind: String,
        connection: ConnectionId,
    },
}

/// [`SpawnRequest`] (bench/test debug path).
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LoggedSpawn {
    pub type_id: u32,
    pub count: usize,
    pub spread: bool,
    pub start: usize,
    pub total: usize,
}

impl LoggedSpawn {
    pub(crate) fn new(request: &SpawnRequest) -> Self {
        Self {
            type_id: request.type_id,
            count: request.count,
            spread: request.spread,
            start: request.start,
            total: request.total,
        }
    }

    #[cfg(test)]
    pub(crate) fn request(&self) -> SpawnRequest {
        SpawnRequest {
            type_id: self.type_id,
            count: self.count,
            spread: self.spread,
            start: self.start,
            total: self.total,
        }
    }
}

/// The fields of a remote ticket the tick consumes.
#[derive(Debug, Serialize, Deserialize)]
pub(crate) struct LoggedTicket {
    pub owner: String,
    pub x: i32,
    pub z: i32,
    pub radius: u32,
}

/// Does this command carry credentials (same roots `tick/auth.rs` refuses to log)?
fn is_auth_command(command: &str) -> bool {
    let root = command
        .trim_start()
        .trim_start_matches('/')
        .split_whitespace()
        .next()
        .unwrap_or("");
    ["register", "login", "changepassword"]
        .iter()
        .any(|auth| root.eq_ignore_ascii_case(auth))
}

/// Text-bearing inputs: refused by the auth gate for a locked session, and the
/// kind of input a password could be typed into.
fn carries_text(input: &PlayerInput) -> bool {
    matches!(
        input,
        PlayerInput::Chat { .. }
            | PlayerInput::Command { .. }
            | PlayerInput::SignUpdate { .. }
            | PlayerInput::EditBook { .. }
            | PlayerInput::RenameItem { .. }
            | PlayerInput::DialogClick { .. }
    )
}

/// `None`: no state effect, not logged. Secrets and unserializable plans
/// become markers. `locked` lists connections the auth gate held locked.
fn logged(input: PlayerInput, locked: &[ConnectionId]) -> Option<LoggedInput> {
    match input {
        PlayerInput::BuildPreviewFence { .. } => None,
        PlayerInput::AuthorizedBuild { authorization } => Some(LoggedInput::Uncovered {
            kind: AUTHORIZED_BUILD.to_owned(),
            connection: authorization.connection(),
        }),
        PlayerInput::Command {
            connection,
            ref command,
            ..
        } if is_auth_command(command) => Some(LoggedInput::Uncovered {
            kind: AUTH_COMMAND.to_owned(),
            connection,
        }),
        input if carries_text(&input) && locked.contains(&input.connection()) => {
            Some(LoggedInput::Uncovered {
                kind: LOCKED_SESSION_TEXT.to_owned(),
                connection: input.connection(),
            })
        }
        input => Some(LoggedInput::Input(input)),
    }
}

/// Writer queue depth in messages (one per non-empty tick).
const QUEUE_DEPTH: usize = 1024;

pub(crate) enum Msg {
    Tick {
        tick: u64,
        inputs: Vec<PlayerInput>,
        spawns: Vec<LoggedSpawn>,
        tickets: Vec<LoggedTicket>,
        locked: Vec<ConnectionId>,
    },
    Gap {
        from: u64,
        to: u64,
    },
    End(u64),
}

/// Handle owned by the tick loop.
pub(crate) struct InputLog {
    tx: SyncSender<Msg>,
    thread: std::thread::JoinHandle<()>,
    /// Dropped ticks not yet reported to the writer.
    gap: Option<(u64, u64)>,
}

impl InputLog {
    /// Create `<world>/input-log/<start_tick>-<unix_ms>.jsonl` and write its header.
    pub(crate) fn open(
        world_dir: &Path,
        start_tick: u64,
        uncovered: Vec<String>,
    ) -> Result<Self, CoreError> {
        let storage = |error: std::io::Error| CoreError::Storage(format!("input log: {error}"));
        let dir = world_dir.join(DIR);
        std::fs::create_dir_all(&dir).map_err(storage)?;
        let unix_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.as_millis());
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(format!("{start_tick}-{unix_ms}.jsonl")))
            .map_err(storage)?;
        let mut out = BufWriter::new(file);
        let header = Line::Header {
            format: FORMAT,
            start_tick,
            uncovered,
        };
        let header =
            serde_json::to_string(&header).map_err(|e| CoreError::Storage(e.to_string()))?;
        writeln!(out, "{header}")
            .and_then(|()| out.flush())
            .map_err(storage)?;
        let (tx, rx) = sync_channel(QUEUE_DEPTH);
        let thread = std::thread::Builder::new()
            .name("mc-core-input-log".to_owned())
            .spawn(move || write_loop(&rx, out))
            .map_err(storage)?;
        Ok(Self {
            tx,
            thread,
            gap: None,
        })
    }

    /// Hand one executed tick's drained inputs to the writer thread. Ticks
    /// that consumed nothing write nothing. Never blocks: when the writer
    /// queue is full the record is dropped and reported as a [`Line::Gap`].
    pub(crate) fn record(
        &mut self,
        tick: u64,
        inputs: Vec<PlayerInput>,
        spawns: Vec<LoggedSpawn>,
        tickets: Vec<LoggedTicket>,
        locked: Vec<ConnectionId>,
    ) {
        if inputs.is_empty() && spawns.is_empty() && tickets.is_empty() {
            return;
        }
        // The gap marker goes first so the file stays in tick order.
        if let Some((from, to)) = self.gap {
            if self.tx.try_send(Msg::Gap { from, to }).is_ok() {
                self.gap = None;
            } else {
                self.gap = Some((from, tick));
                return;
            }
        }
        let message = Msg::Tick {
            tick,
            inputs,
            spawns,
            tickets,
            locked,
        };
        // A dead writer already reported itself; the tick never waits on it.
        if let Err(TrySendError::Full(_)) = self.tx.try_send(message) {
            if self.gap.is_none() {
                tracing::warn!(tick, "input log writer queue full; dropping records");
            }
            self.gap = Some((tick, tick));
        }
    }

    /// Clean stop: report any gap, write the end marker, wait for the flush.
    /// Blocks on the writer (shutdown has no tick left to protect).
    pub(crate) fn finish(self, tick: u64) {
        if let Some((from, to)) = self.gap {
            let _ = self.tx.send(Msg::Gap { from, to });
        }
        let _ = self.tx.send(Msg::End(tick));
        drop(self.tx);
        let _ = self.thread.join();
    }
}

#[cfg(test)]
impl Msg {
    pub(crate) fn describe(&self) -> String {
        match self {
            Self::Tick { tick, .. } => format!("tick {tick}"),
            Self::Gap { from, to } => format!("gap {from}..={to}"),
            Self::End(tick) => format!("end {tick}"),
        }
    }
}

#[cfg(test)]
impl InputLog {
    /// A log with no writer thread: the test owns the receiving end, so it
    /// controls exactly when the queue is full.
    pub(crate) fn detached(depth: usize) -> (Self, Receiver<Msg>) {
        let (tx, rx) = sync_channel(depth);
        let thread = std::thread::spawn(|| {});
        (
            Self {
                tx,
                thread,
                gap: None,
            },
            rx,
        )
    }
}

fn encode(line: &Line) -> Option<String> {
    let text = serde_json::to_string(line).ok()?;
    // Non-finite floats serialize as `null` and would not read back; real
    // `None`s are `null` too, so only lines containing one are re-parsed.
    if text.contains("null") {
        serde_json::from_str::<Line>(&text).ok()?;
    }
    Some(text)
}

fn tick_line(
    tick: u64,
    inputs: Vec<PlayerInput>,
    spawns: Vec<LoggedSpawn>,
    tickets: Vec<LoggedTicket>,
    locked: &[ConnectionId],
) -> String {
    let record = TickRecord {
        tick,
        spawns,
        tickets,
        inputs: inputs
            .into_iter()
            .filter_map(|input| logged(input, locked))
            .collect(),
    };
    let line = Line::Tick(record);
    encode(&line).unwrap_or_else(|| {
        let Line::Tick(mut record) = line else {
            unreachable!()
        };
        record.inputs = vec![LoggedInput::Uncovered {
            kind: UNSERIALIZABLE.to_owned(),
            connection: ConnectionId(0),
        }];
        serde_json::to_string(&Line::Tick(record)).expect("marker line serializes")
    })
}

fn write_loop(rx: &Receiver<Msg>, mut out: BufWriter<std::fs::File>) {
    let mut failed = false;
    loop {
        let msg = match rx.try_recv() {
            Ok(msg) => msg,
            Err(TryRecvError::Empty) => {
                if !failed && let Err(error) = out.flush() {
                    tracing::error!(%error, "input log write failed; logging stopped");
                    failed = true;
                }
                match rx.recv() {
                    Ok(msg) => msg,
                    Err(_) => break,
                }
            }
            Err(TryRecvError::Disconnected) => break,
        };
        if failed {
            continue;
        }
        let text = match msg {
            Msg::Tick {
                tick,
                inputs,
                spawns,
                tickets,
                locked,
            } => tick_line(tick, inputs, spawns, tickets, &locked),
            Msg::Gap { from, to } => {
                serde_json::to_string(&Line::Gap { from, to }).expect("gap line")
            }
            Msg::End(tick) => serde_json::to_string(&Line::End { tick }).expect("end line"),
        };
        if let Err(error) = writeln!(out, "{text}") {
            tracing::error!(%error, "input log write failed; logging stopped");
            failed = true;
        }
    }
    if !failed {
        let _ = out.flush();
    }
}

/// `ScriptClientViewKey` as its validated string.
pub(crate) mod view_key {
    use mc_script::ScriptClientViewKey;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        key: &ScriptClientViewKey,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(key.as_str())
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<ScriptClientViewKey, D::Error> {
        let text = String::deserialize(deserializer)?;
        ScriptClientViewKey::try_new(&text).map_err(|error| D::Error::custom(format!("{error:?}")))
    }
}

/// `Option<Tag>` as hex of its anonymous network NBT encoding.
pub(crate) mod nbt_option {
    use mc_nbt::Tag;
    use serde::de::Error;
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub(crate) fn serialize<S: Serializer>(
        tag: &Option<Tag>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        let hex = match tag {
            None => None,
            Some(tag) => {
                let mut bytes = Vec::new();
                mc_nbt::write_network_value(&mut bytes, tag)
                    .map_err(|error| serde::ser::Error::custom(error.to_string()))?;
                Some(
                    bytes
                        .iter()
                        .map(|byte| format!("{byte:02x}"))
                        .collect::<String>(),
                )
            }
        };
        hex.serialize(serializer)
    }

    pub(crate) fn deserialize<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<Tag>, D::Error> {
        let Some(hex) = Option::<String>::deserialize(deserializer)? else {
            return Ok(None);
        };
        if hex.len() % 2 != 0 || !hex.is_ascii() {
            return Err(D::Error::custom("odd or non-ascii NBT hex"));
        }
        let bytes = (0..hex.len())
            .step_by(2)
            .map(|at| u8::from_str_radix(&hex[at..at + 2], 16))
            .collect::<Result<Vec<u8>, _>>()
            .map_err(D::Error::custom)?;
        mc_nbt::read_network_value(&mut bytes.as_slice())
            .map(Some)
            .map_err(|error| D::Error::custom(error.to_string()))
    }
}
