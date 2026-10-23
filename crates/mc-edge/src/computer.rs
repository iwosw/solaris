//! Connection-bound operator terminal; see ADR 0018. Never runs on the tick.
use std::{net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc, time::Duration};

use mc_core::boundary::{ConnectionId, EntityId, TickOutput};
use mc_core::dimensions::Dimension;
use mc_data::Identifier;
use mc_protocol::packets::{CustomPayload, Packet, play::ServerboundCustomPayload};
use serde::Deserialize;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, ChildStdin, ChildStdout},
    sync::mpsc,
};

use crate::server::EdgeServer;

const MAX_INPUT: usize = 4096;
const MAX_OUTPUT: usize = 16384;
const CHANNEL: &str = "solaris:computer";

/// Explicit host-owned configuration, never supplied by a client or guest.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ComputerConfig {
    pub enabled: bool,
    pub local_only: bool,
    /// Empty selects built-in echo. Otherwise executable + literal arguments.
    pub command: Vec<String>,
    pub working_directory: PathBuf,
}
impl Default for ComputerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            local_only: true,
            command: Vec::new(),
            working_directory: PathBuf::from("."),
        }
    }
}
impl ComputerConfig {
    pub(crate) fn validate(&self) -> anyhow::Result<()> {
        if self.enabled && !self.command.is_empty() {
            anyhow::ensure!(
                std::path::Path::new(&self.command[0]).is_absolute(),
                "computer.command executable must be absolute"
            );
            anyhow::ensure!(
                self.working_directory.is_absolute(),
                "computer.working_directory must be absolute for a process bridge"
            );
        }
        Ok(())
    }
}

#[derive(Debug, PartialEq)]
enum Request {
    Open { session: u32, at: [i32; 3] },
    Input { session: u32, text: String },
    Close { session: u32 },
}
impl Request {
    fn session(&self) -> u32 {
        match self {
            Self::Open { session, .. } | Self::Input { session, .. } | Self::Close { session } => {
                *session
            }
        }
    }
}
fn decode(bytes: &[u8]) -> anyhow::Result<Request> {
    anyhow::ensure!(
        (6..=MAX_INPUT + 6).contains(&bytes.len()) && bytes[0] == 1,
        "invalid computer header"
    );
    let session = u32::from_be_bytes(bytes[2..6].try_into()?);
    anyhow::ensure!(session != 0, "zero computer session");
    let body = &bytes[6..];
    match bytes[1] {
        0 if body.len() == 12 => Ok(Request::Open {
            session,
            at: [
                i32::from_be_bytes(body[0..4].try_into()?),
                i32::from_be_bytes(body[4..8].try_into()?),
                i32::from_be_bytes(body[8..12].try_into()?),
            ],
        }),
        1 => {
            let text = std::str::from_utf8(body)?;
            anyhow::ensure!(
                !text.chars().any(char::is_control),
                "computer input must be a single plain-text line"
            );
            Ok(Request::Input {
                session,
                text: text.to_owned(),
            })
        }
        2 if body.is_empty() => Ok(Request::Close { session }),
        _ => anyhow::bail!("invalid computer request"),
    }
}

pub(crate) fn payload(body: &[u8]) -> anyhow::Result<Option<Vec<u8>>> {
    let mut body = body;
    let packet = ServerboundCustomPayload::decode(&mut body)?;
    if let CustomPayload::Unknown { channel, payload } = packet.payload
        && channel.as_str() == CHANNEL
    {
        anyhow::ensure!(body.is_empty(), "trailing computer packet bytes");
        return Ok(Some(payload));
    }
    Ok(None)
}

fn permitted(
    cfg: &ComputerConfig,
    local: bool,
    authenticated: bool,
    level: u8,
    locked: bool,
) -> bool {
    cfg.enabled
        && level >= 4
        && !locked
        && if cfg.local_only {
            local
        } else {
            local || authenticated
        }
}
fn allowed(server: &EdgeServer, id: ConnectionId, peer: SocketAddr) -> bool {
    let proxy = server.cfg.proxy.mode != crate::config::ProxyMode::None;
    permitted(
        &server.cfg.computer,
        peer.ip().is_loopback() && !proxy,
        server.cfg.auth.online_mode || proxy,
        server.admin.session_level(id),
        server.hub.session_locked(id),
    )
}

#[derive(Clone, Copy)]
struct Target {
    session: u32,
    at: [i32; 3],
    dimension: Dimension,
}
fn target_dimension(
    output: &TickOutput,
    entity: EntityId,
    at: [i32; 3],
    carrier: u32,
) -> Option<Dimension> {
    std::iter::once(output)
        .chain(output.snapshot.dimensions.values())
        .find_map(|out| {
            let snapshot = &out.snapshot;
            let player = snapshot.entities.get(entity)?;
            let distance: f64 = at
                .iter()
                .zip(player.position)
                .map(|(&block, player)| (f64::from(block) + 0.5 - player).powi(2))
                .sum();
            if !distance.is_finite() || distance > 36.0 || player.appearance.health <= 0.0 {
                return None;
            }
            let key = (at[0].div_euclid(16), at[2].div_euclid(16));
            let index = snapshot
                .chunks
                .binary_search_by_key(&key, |chunk| (chunk.pos.x, chunk.pos.z))
                .ok()?;
            let state = snapshot.chunks[index].chunk.get_block(
                at[0].rem_euclid(16) as u8,
                at[1],
                at[2].rem_euclid(16) as u8,
            )?;
            (state.0 == carrier).then_some(snapshot.dimension)
        })
}

struct Process {
    _child: Child,
    input: ChildStdin,
    output: ChildStdout,
}
fn spawn(cfg: &ComputerConfig) -> anyhow::Result<Option<Process>> {
    let Some(program) = cfg.command.first() else {
        return Ok(None);
    };
    let mut child = tokio::process::Command::new(program)
        .args(&cfg.command[1..])
        .current_dir(&cfg.working_directory)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    Ok(Some(Process {
        input: child.stdin.take().expect("piped stdin"),
        output: child.stdout.take().expect("piped stdout"),
        _child: child,
    }))
}

/// Dropping a Play connection aborts its worker, dropping/killing its process.
pub(crate) struct Connection {
    ingress: mpsc::Sender<Request>,
    task: tokio::task::AbortHandle,
}
impl Drop for Connection {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Connection {
    pub(crate) fn start(
        server: Arc<EdgeServer>,
        id: ConnectionId,
        entity: EntityId,
        peer: SocketAddr,
    ) -> Self {
        let (ingress, rx) = mpsc::channel(16);
        let task = tokio::spawn(async move {
            if let Err(error) = run(&server, id, entity, peer, rx).await {
                tracing::warn!(connection = id.0, %error, "computer session stopped");
                let _ = server.kick(
                    id,
                    serde_json::json!({"text":"Computer session transport failed"}),
                );
            }
        })
        .abort_handle();
        Self { ingress, task }
    }
    pub(crate) fn receive(&self, bytes: &[u8]) -> anyhow::Result<()> {
        self.ingress.try_send(decode(bytes)?)?;
        Ok(())
    }
}
fn reply(
    server: &EdgeServer,
    id: ConnectionId,
    session: u32,
    kind: u8,
    text: &str,
) -> anyhow::Result<()> {
    anyhow::ensure!(text.len() <= MAX_OUTPUT, "computer output too long");
    let mut bytes = Vec::with_capacity(6 + text.len());
    bytes.extend([1, kind]);
    bytes.extend(session.to_be_bytes());
    bytes.extend(text.as_bytes());
    let frame = crate::lod::frame(Identifier::parse(CHANNEL)?, bytes)?;
    let handles = server
        .connections
        .get(id)
        .ok_or_else(|| anyhow::anyhow!("computer connection closed"))?;
    handles.reliable_tx.try_send(frame)?;
    Ok(())
}

async fn run(
    server: &EdgeServer,
    id: ConnectionId,
    entity: EntityId,
    peer: SocketAddr,
    mut rx: mpsc::Receiver<Request>,
) -> anyhow::Result<()> {
    let carrier = server
        .wire
        .blocks
        .block(&Identifier::parse("minecraft:iron_block")?)
        .ok_or_else(|| anyhow::anyhow!("computer carrier unavailable"))?
        .default
        .0;
    let mut ticks = server.hub.subscribe();
    let mut latest: Option<TickOutput> = None;
    let mut target: Option<Target> = None;
    let mut process: Option<Process> = None;
    let mut buffer = [0u8; 4096];
    let mut pending_utf8 = Vec::new();
    loop {
        tokio::select! {
            biased;
            tick = ticks.recv() => {
                match tick {
                    Ok(tick) => {
                        if let Some(current) = target
                            && (!allowed(server, id, peer) || target_dimension(&tick, entity, current.at, carrier) != Some(current.dimension))
                        {
                            process = None; target = None; pending_utf8.clear();
                            reply(server, id, current.session, 2, "Computer access ended (permission, block or reach changed).")?;
                        }
                        latest = Some(tick);
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        latest = None;
                        process = None;
                        pending_utf8.clear();
                        if let Some(current) = target.take() {
                            reply(server, id, current.session, 2, "Computer closed: native state publication was missed.")?;
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => return Ok(()),
                }
            }
            request = rx.recv() => {
                let Some(request) = request else { return Ok(()); };
                let session = request.session();
                if !allowed(server, id, peer) {
                    process = None; target = None; pending_utf8.clear();
                    reply(server, id, session, 2, "Computer requires an unlocked level-4 operator and an allowed authenticated connection.")?;
                    continue;
                }
                match request {
                    Request::Open { at, .. } => {
                        process = None; target = None; pending_utf8.clear();
                        let dimension = latest.as_ref().and_then(|tick| target_dimension(tick, entity, at, carrier));
                        let Some(dimension) = dimension else {
                            reply(server, id, session, 2, "Use a nearby loaded iron block; native player state must be available.")?;
                            continue;
                        };
                        match spawn(&server.cfg.computer) {
                            Ok(child) => {
                                process = child; target = Some(Target { session, at, dimension });
                                reply(server, id, session, 0, "Computer connected. Plain text lines; Escape closes the process.")?;
                            }
                            Err(error) => {
                                tracing::warn!(%error, "computer bridge launch failed");
                                reply(server, id, session, 2, "Configured bridge could not start; consult the server operator.")?;
                            }
                        }
                    }
                    Request::Input { text, .. } => {
                        let valid = target.is_some_and(|t| t.session == session && latest.as_ref().and_then(|tick| target_dimension(tick, entity, t.at, carrier)) == Some(t.dimension));
                        if !valid {
                            process = None; target = None; pending_utf8.clear();
                            reply(server, id, session, 2, "No active computer session.")?;
                            continue;
                        }
                        if let Some(child) = &mut process {
                            let sent = tokio::time::timeout(Duration::from_secs(2), async {
                                child.input.write_all(text.as_bytes()).await?;
                                child.input.write_all(b"\n").await
                            }).await;
                            if !matches!(sent, Ok(Ok(()))) {
                                process = None; target = None; pending_utf8.clear();
                                reply(server, id, session, 2, "Bridge input closed or timed out.")?;
                            }
                        } else {
                            reply(server, id, session, 1, &format!("echo: {text}\n"))?;
                        }
                    }
                    Request::Close { .. } => {
                        if target.is_some_and(|t| t.session == session) {
                            process = None; target = None; pending_utf8.clear();
                            reply(server, id, session, 2, "Computer closed.")?;
                        }
                    }
                }
            }
            output = async { match &mut process { Some(child) => child.output.read(&mut buffer).await, None => std::future::pending().await } } => {
                let Some(current) = target else { continue; };
                if !allowed(server, id, peer) {
                    process = None; target = None; pending_utf8.clear();
                    reply(server, id, current.session, 2, "Computer permission revoked.")?;
                    continue;
                }
                match output {
                    Ok(count) if count > 0 => {
                        pending_utf8.extend_from_slice(&buffer[..count]);
                        let end = match std::str::from_utf8(&pending_utf8) {
                            Err(error) if error.error_len().is_none() => error.valid_up_to(),
                            _ => pending_utf8.len(),
                        };
                        reply(server, id, current.session, 1, &String::from_utf8_lossy(&pending_utf8[..end]))?;
                        pending_utf8.drain(..end);
                    }
                    _ => {
                        if !pending_utf8.is_empty() { reply(server, id, current.session, 1, &String::from_utf8_lossy(&pending_utf8))?; }
                        process = None; target = None; pending_utf8.clear();
                        reply(server, id, current.session, 2, "Bridge exited or closed stdout.")?;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "computer_tests.rs"]
mod tests;
