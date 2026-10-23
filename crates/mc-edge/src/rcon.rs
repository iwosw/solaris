//! RCON wire from local 26.1.2 javap RconClient: little-endian length/id/type,
//! two NUL terminators, auth type 3, command type 2, response type 0.
use crate::server::EdgeServer;
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

const MAX_BODY: usize = 4096;
const IO_TIMEOUT: Duration = Duration::from_secs(30);

pub(super) async fn serve(listener: TcpListener, server: Arc<EdgeServer>) -> anyhow::Result<()> {
    let mut clients = tokio::task::JoinSet::new();
    loop {
        tokio::select! {
            _ = clients.join_next(), if !clients.is_empty() => {}
            accepted = listener.accept() => {
                let (stream, _) = accepted?;
                if clients.len() >= 16 { continue; }
                let server = Arc::clone(&server);
                clients.spawn(async move {
                    if let Err(error) = session(stream, server).await { tracing::debug!(%error, "RCON connection closed"); }
                });
            }
        }
    }
}

struct Request {
    id: i32,
    kind: i32,
    body: String,
}

async fn read(stream: &mut TcpStream) -> anyhow::Result<Request> {
    let size = stream.read_i32_le().await?;
    anyhow::ensure!(
        (10..=(MAX_BODY as i32 + 10)).contains(&size),
        "invalid RCON frame length"
    );
    let mut bytes = vec![0; size as usize];
    stream.read_exact(&mut bytes).await?;
    anyhow::ensure!(
        bytes[bytes.len() - 2..] == [0, 0],
        "invalid RCON terminators"
    );
    let body = &bytes[8..bytes.len() - 2];
    anyhow::ensure!(!body.contains(&0), "embedded RCON NUL");
    Ok(Request {
        id: i32::from_le_bytes(bytes[..4].try_into()?),
        kind: i32::from_le_bytes(bytes[4..8].try_into()?),
        body: std::str::from_utf8(body)?.into(),
    })
}

async fn send(stream: &mut TcpStream, id: i32, kind: i32, body: &str) -> anyhow::Result<()> {
    let mut bytes = Vec::with_capacity(body.len() + 14);
    bytes.extend_from_slice(&(body.len() as i32 + 10).to_le_bytes());
    bytes.extend_from_slice(&id.to_le_bytes());
    bytes.extend_from_slice(&kind.to_le_bytes());
    bytes.extend_from_slice(body.as_bytes());
    bytes.extend_from_slice(&[0, 0]);
    tokio::time::timeout(IO_TIMEOUT, stream.write_all(&bytes)).await??;
    Ok(())
}

fn password_matches(expected: &str, supplied: &str) -> bool {
    use sha2::{Digest, Sha256};
    let expected = Sha256::digest(expected.as_bytes());
    let supplied = Sha256::digest(supplied.as_bytes());
    expected
        .iter()
        .zip(supplied.iter())
        .fold(0u8, |diff, (a, b)| diff | (a ^ b))
        == 0
}

async fn session(mut stream: TcpStream, server: Arc<EdgeServer>) -> anyhow::Result<()> {
    let mut authenticated = false;
    loop {
        let request = tokio::time::timeout(IO_TIMEOUT, read(&mut stream)).await??;
        if request.kind == 3 {
            authenticated = password_matches(&server.cfg.rcon.password, &request.body);
            send(
                &mut stream,
                if authenticated { request.id } else { -1 },
                2,
                "",
            )
            .await?;
            if !authenticated {
                return Ok(());
            }
        } else if !authenticated {
            send(&mut stream, -1, 2, "").await?;
            return Ok(());
        } else {
            let response = if request.kind == 2 {
                match crate::console::remote(&server, &request.body).await {
                    Ok(response) => response,
                    Err(error) => format!("Command failed: {error:#}"),
                }
            } else {
                format!("Unknown request {:x}", request.kind)
            };
            let mut remaining = response.as_str();
            loop {
                let mut end = remaining.len().min(MAX_BODY);
                while !remaining.is_char_boundary(end) {
                    end -= 1;
                }
                send(&mut stream, request.id, 0, &remaining[..end]).await?;
                remaining = &remaining[end..];
                if remaining.is_empty() {
                    break;
                }
            }
        }
    }
}

#[cfg(test)]
#[path = "rcon_tests.rs"]
mod tests;
