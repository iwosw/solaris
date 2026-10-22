//! Minimal Minecraft Server Management Protocol (MSMP) endpoint.
//!
//! This is deliberately a small JSON-RPC 2.0 WebSocket server rather than a
//! second HTTP framework. It implements the management services Solaris owns:
//! players, access lists, operators, server state, save/stop, and system
//! messages. The method names and DTO field names follow the local 26.1.2
//! vanilla `net.minecraft.server.jsonrpc` schema.

use std::sync::Arc;

use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

use crate::server::EdgeServer;

const MAX_HEAD_BYTES: usize = 8 * 1024;
const MAX_MESSAGE_BYTES: usize = 64 * 1024;
const WS_GUID: &str = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11";

pub async fn serve(addr: String, port: u16, token: String, server: Arc<EdgeServer>) {
    let target = format!("{addr}:{port}");
    let Ok(listener) = TcpListener::bind(&target).await else {
        tracing::warn!(target = %target, "MSMP bind failed");
        return;
    };
    tracing::info!(target = %target, "MSMP listening");
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        let server = Arc::clone(&server);
        let token = token.clone();
        tokio::spawn(async move {
            let _ = serve_connection(stream, &token, &server).await;
        });
    }
}

async fn serve_connection(
    mut stream: TcpStream,
    token: &str,
    server: &EdgeServer,
) -> anyhow::Result<()> {
    let head = read_head(&mut stream).await?;
    let Some((key, bearer)) = upgrade(&head) else {
        write_http(&mut stream, "400 Bad Request", b"bad websocket upgrade").await?;
        return Ok(());
    };
    if !same_token(token, bearer.as_deref()) {
        write_http(&mut stream, "401 Unauthorized", b"unauthorized").await?;
        return Ok(());
    }
    let accept = websocket_accept(&key);
    stream
        .write_all(
            format!(
                "HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: {accept}\r\n\r\n"
            )
            .as_bytes(),
        )
        .await?;
    stream.flush().await?;

    while let Some(message) = read_text_frame(&mut stream).await? {
        let response = dispatch(&message, server).await;
        write_text_frame(&mut stream, &response.to_string()).await?;
    }
    Ok(())
}

async fn dispatch(raw: &str, server: &EdgeServer) -> Value {
    let Ok(request) = serde_json::from_str::<Value>(raw) else {
        return error(Value::Null, -32700, "Parse error");
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    if request.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return error(id, -32600, "Invalid Request");
    }
    let Some(method) = request.get("method").and_then(Value::as_str) else {
        return error(id, -32600, "Invalid Request");
    };
    let params = request.get("params").cloned().unwrap_or_else(|| json!([]));
    let result = match method {
        "rpc.discover" => Ok(discovery()),
        "minecraft:players" => Ok(players(server)),
        "minecraft:players/kick" => mutate_players(server, &params, "kick").await,
        "minecraft:allowlist" => Ok(allowlist(server)),
        "minecraft:allowlist/add" => mutate_players(server, &params, "whitelist add").await,
        "minecraft:allowlist/remove" => mutate_players(server, &params, "whitelist remove").await,
        "minecraft:allowlist/clear" => clear_allowlist(server).await,
        "minecraft:allowlist/set" => set_allowlist(server, &params).await,
        "minecraft:bans" => Ok(bans(server)),
        "minecraft:bans/add" => mutate_players(server, &params, "ban").await,
        "minecraft:bans/remove" => mutate_players(server, &params, "pardon").await,
        "minecraft:operators" => Ok(operators(server)),
        "minecraft:operators/add" => mutate_players(server, &params, "op").await,
        "minecraft:operators/remove" => mutate_players(server, &params, "deop").await,
        "minecraft:server/status" => Ok(status(server)),
        "minecraft:server/save" => run_command(server, "save-all").await.map(|_| json!(true)),
        "minecraft:server/stop" => run_command(server, "stop").await.map(|_| json!(true)),
        "minecraft:server/system_message" => system_message(server, &params).await,
        _ => return error(id, -32601, "Method not found"),
    };
    match result {
        Ok(result) => json!({"jsonrpc":"2.0", "id":id, "result":result}),
        Err(message) => error(id, -32602, &message),
    }
}

fn discovery() -> Value {
    json!({
        "version": {"name":"26.1.2", "protocol":775},
        "methods": [
            "minecraft:players", "minecraft:players/kick",
            "minecraft:allowlist", "minecraft:allowlist/add", "minecraft:allowlist/remove", "minecraft:allowlist/set", "minecraft:allowlist/clear",
            "minecraft:bans", "minecraft:bans/add", "minecraft:bans/remove",
            "minecraft:operators", "minecraft:operators/add", "minecraft:operators/remove",
            "minecraft:server/status", "minecraft:server/save", "minecraft:server/stop", "minecraft:server/system_message"
        ]
    })
}

fn players(server: &EdgeServer) -> Value {
    Value::Array(
        server
            .connections
            .roster()
            .into_iter()
            .map(|name| player_dto(server, &name))
            .collect(),
    )
}

fn player_dto(server: &EdgeServer, name: &str) -> Value {
    let mut player = json!({"name":name});
    if let Some((_, uuid, _, _)) = server.admin.find(name) {
        player["id"] = json!(uuid);
    }
    player
}

fn allowlist(server: &EdgeServer) -> Value {
    Value::Array(
        server
            .admin
            .whitelist_names()
            .into_iter()
            .map(|name| player_dto(server, &name))
            .collect(),
    )
}

fn bans(server: &EdgeServer) -> Value {
    let (players, ips) = server.admin.bans();
    json!({"players":players, "ips":ips})
}

fn operators(server: &EdgeServer) -> Value {
    Value::Array(
        server
            .admin
            .operator_names()
            .into_iter()
            .map(|name| json!({"player":player_dto(server, &name), "permissionLevel":4, "bypassesPlayerLimit":false}))
            .collect(),
    )
}

fn status(server: &EdgeServer) -> Value {
    let stats = server.dashboard.snapshot();
    json!({
        "started": true,
        "players": players(server),
        "version": {"name":"26.1.2", "protocol":775},
        "tps": stats.tps,
        "tick": stats.simulation_tick,
    })
}

async fn mutate_players(
    server: &EdgeServer,
    params: &Value,
    command: &str,
) -> Result<Value, String> {
    let entries = params
        .as_array()
        .and_then(|params| params.first())
        .and_then(Value::as_array)
        .ok_or_else(|| "expected one array parameter".to_owned())?;
    for entry in entries {
        let player = entry.get("player").unwrap_or(entry);
        let name = player
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| "player name is required".to_owned())?;
        let reason = entry
            .get("message")
            .and_then(|message| message.get("literal"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        let full = if reason.is_empty() {
            format!("{command} {name}")
        } else {
            format!("{command} {name} {reason}")
        };
        run_command(server, &full).await?;
    }
    Ok(match command {
        "kick" => players(server),
        command if command.starts_with("whitelist") => allowlist(server),
        command if command.starts_with("ban") || command == "pardon" => bans(server),
        _ => operators(server),
    })
}

async fn clear_allowlist(server: &EdgeServer) -> Result<Value, String> {
    for name in server.admin.whitelist_names() {
        run_command(server, &format!("whitelist remove {name}")).await?;
    }
    Ok(allowlist(server))
}

async fn set_allowlist(server: &EdgeServer, params: &Value) -> Result<Value, String> {
    clear_allowlist(server).await?;
    mutate_players(server, params, "whitelist add").await
}

async fn system_message(server: &EdgeServer, params: &Value) -> Result<Value, String> {
    let message = params
        .as_array()
        .and_then(|params| params.first())
        .and_then(|message| message.get("message"))
        .and_then(|message| message.get("literal"))
        .and_then(Value::as_str)
        .ok_or_else(|| "message.literal is required".to_owned())?;
    run_command(server, &format!("say {message}")).await?;
    Ok(json!(true))
}

async fn run_command(server: &EdgeServer, command: &str) -> Result<(), String> {
    crate::console::execute(server, command)
        .await
        .map_err(|error| error.to_string())
}

fn error(id: Value, code: i32, message: &str) -> Value {
    json!({"jsonrpc":"2.0", "id":id, "error":{"code":code, "message":message}})
}

async fn read_head(stream: &mut TcpStream) -> anyhow::Result<String> {
    let mut bytes = Vec::with_capacity(512);
    while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
        if bytes.len() >= MAX_HEAD_BYTES {
            anyhow::bail!("request head exceeds limit");
        }
        let mut byte = [0u8; 1];
        if stream.read(&mut byte).await? == 0 {
            anyhow::bail!("connection closed before request head");
        }
        bytes.push(byte[0]);
    }
    String::from_utf8(bytes).map_err(Into::into)
}

fn upgrade(head: &str) -> Option<(String, Option<String>)> {
    let request = head.lines().next()?;
    if !request.starts_with("GET ") {
        return None;
    }
    let mut key = None;
    let mut bearer = None;
    let mut websocket = false;
    for line in head.lines().skip(1).filter(|line| !line.trim().is_empty()) {
        let (name, value) = line.split_once(':')?;
        if name.eq_ignore_ascii_case("sec-websocket-key") {
            key = Some(value.trim().to_owned());
        } else if name.eq_ignore_ascii_case("upgrade") {
            websocket = value.trim().eq_ignore_ascii_case("websocket");
        } else if name.eq_ignore_ascii_case("authorization") {
            bearer = value.trim().strip_prefix("Bearer ").map(str::to_owned);
        }
    }
    websocket.then_some((key?, bearer))
}

fn same_token(expected: &str, actual: Option<&str>) -> bool {
    let Some(actual) = actual else {
        return false;
    };
    expected.len() == actual.len()
        && expected
            .bytes()
            .zip(actual.bytes())
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

fn websocket_accept(key: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(key.as_bytes());
    hasher.update(WS_GUID.as_bytes());
    base64(&hasher.finalize())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for group in bytes.chunks(3) {
        let value = (u32::from(group[0]) << 16)
            | (u32::from(*group.get(1).unwrap_or(&0)) << 8)
            | u32::from(*group.get(2).unwrap_or(&0));
        output.push(TABLE[(value >> 18) as usize & 63] as char);
        output.push(TABLE[(value >> 12) as usize & 63] as char);
        output.push(if group.len() > 1 {
            TABLE[(value >> 6) as usize & 63] as char
        } else {
            '='
        });
        output.push(if group.len() > 2 {
            TABLE[value as usize & 63] as char
        } else {
            '='
        });
    }
    output
}

async fn read_text_frame(stream: &mut TcpStream) -> anyhow::Result<Option<String>> {
    let mut header = [0u8; 2];
    if stream.read_exact(&mut header).await.is_err() {
        return Ok(None);
    }
    if header[0] & 0x80 == 0 {
        anyhow::bail!("fragmented WebSocket frames are unsupported");
    }
    match header[0] & 0x0f {
        0x8 => return Ok(None),
        0x1 => {}
        _ => anyhow::bail!("expected text WebSocket frame"),
    }
    if header[1] & 0x80 == 0 {
        anyhow::bail!("client WebSocket frame must be masked");
    }
    let mut length = u64::from(header[1] & 0x7f);
    if length == 126 {
        let mut extended = [0u8; 2];
        stream.read_exact(&mut extended).await?;
        length = u64::from(u16::from_be_bytes(extended));
    } else if length == 127 {
        let mut extended = [0u8; 8];
        stream.read_exact(&mut extended).await?;
        length = u64::from_be_bytes(extended);
    }
    if length as usize > MAX_MESSAGE_BYTES {
        anyhow::bail!("WebSocket message exceeds limit");
    }
    let mut mask = [0u8; 4];
    stream.read_exact(&mut mask).await?;
    let mut body = vec![0u8; length as usize];
    stream.read_exact(&mut body).await?;
    for (index, byte) in body.iter_mut().enumerate() {
        *byte ^= mask[index % mask.len()];
    }
    Ok(Some(String::from_utf8(body)?))
}

async fn write_text_frame(stream: &mut TcpStream, body: &str) -> anyhow::Result<()> {
    let bytes = body.as_bytes();
    if bytes.len() > MAX_MESSAGE_BYTES {
        anyhow::bail!("JSON-RPC response exceeds limit");
    }
    stream.write_u8(0x81).await?;
    match bytes.len() {
        0..=125 => stream.write_u8(bytes.len() as u8).await?,
        126..=65_535 => {
            stream.write_u8(126).await?;
            stream.write_u16(bytes.len() as u16).await?;
        }
        _ => {
            stream.write_u8(127).await?;
            stream.write_u64(bytes.len() as u64).await?;
        }
    }
    stream.write_all(bytes).await?;
    stream.flush().await?;
    Ok(())
}

async fn write_http(stream: &mut TcpStream, status: &str, body: &[u8]) -> anyhow::Result<()> {
    stream
        .write_all(
            format!(
                "HTTP/1.1 {status}\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                body.len()
            )
            .as_bytes(),
        )
        .await?;
    stream.write_all(body).await?;
    stream.flush().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn websocket_handshake_matches_rfc_example_and_requires_bearer() {
        assert_eq!(
            websocket_accept("dGhlIHNhbXBsZSBub25jZQ=="),
            "s3pPLMBiTxaQ9kYGzzhZRbK+xOo="
        );
        let request = "GET /msmp HTTP/1.1\r\nUpgrade: websocket\r\n\
                       Connection: Upgrade\r\nSec-WebSocket-Key: key\r\n\
                       Authorization: Bearer secret\r\n\r\n";
        assert_eq!(
            upgrade(request),
            Some(("key".to_owned(), Some("secret".to_owned())))
        );
        assert!(same_token("secret", Some("secret")));
        assert!(!same_token("secret", Some("wrong")));
        assert!(!same_token("secret", None));
    }

    #[test]
    fn discovery_advertises_the_supported_vanilla_management_services() {
        let methods = discovery()["methods"].as_array().unwrap().clone();
        for method in [
            "minecraft:players",
            "minecraft:allowlist/add",
            "minecraft:bans/add",
            "minecraft:operators/add",
            "minecraft:server/status",
            "minecraft:server/save",
            "minecraft:server/stop",
            "minecraft:server/system_message",
        ] {
            assert!(
                methods.iter().any(|value| value == method),
                "missing {method}"
            );
        }
    }
}
