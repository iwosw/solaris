//! Startup-only Java properties import. Explicit TOML leaves always win.
use std::path::Path;

use anyhow::Context;
use toml::Value;

pub(crate) fn load(text: &str, origin: &Path) -> anyhow::Result<crate::config::EdgeConfig> {
    let mut table: toml::Table = toml::from_str(text)?;
    let directory = origin.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let explicit = table.get("properties").and_then(Value::as_table)
        .and_then(|t| t.get("file")).and_then(Value::as_str);
    let path = directory.join(explicit.unwrap_or("server.properties"));
    match std::fs::read(&path) {
        Ok(bytes) => {
            // java.util.Properties.load(InputStream) uses ISO-8859-1, not UTF-8.
            let source: String = bytes.into_iter().map(char::from).collect();
            overlay(&mut table, &source).with_context(|| format!("importing {}", path.display()))?;
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound && explicit.is_none() => {}
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    }
    crate::config::EdgeConfig::from_toml(&toml::to_string(&table)?, Some(origin))
}

fn overlay(table: &mut toml::Table, text: &str) -> anyhow::Result<()> {
    for (key, value) in parse(text)? {
        let (section, leaf, value) = match key.as_str() {
            "motd" => ("server", "motd", Value::String(value)),
            "max-players" => ("server", "max_players", integer(&key, &value)?),
            "enable-command-block" => ("server", "enable_command_block", boolean(&key, &value)?),
            "view-distance" => ("server", "view_distance", integer(&key, &value)?),
            "simulation-distance" => ("server", "simulation_distance", integer(&key, &value)?),
            "accepts-transfers" => ("server", "accepts_transfers", boolean(&key, &value)?),
            "pause-when-empty-seconds" => ("server", "pause_when_empty_seconds", integer(&key, &value)?),
            "white-list" => ("server", "whitelist", boolean(&key, &value)?),
            "server-ip" => ("network", "bind_address", Value::String(if value.is_empty() { "0.0.0.0".into() } else { value })),
            "server-port" => ("network", "port", integer(&key, &value)?),
            "online-mode" => ("auth", "online_mode", boolean(&key, &value)?),
            "level-name" => ("data", "world_dir", Value::String(value)),
            "level-seed" if value.is_empty() => continue,
            "level-seed" => ("data", "seed", Value::Integer(value.parse().unwrap_or_else(|_| {
                i64::from(value.encode_utf16().fold(0i32, |hash, c| hash.wrapping_mul(31).wrapping_add(i32::from(c))))
            }))),
            "enable-rcon" => ("rcon", "enabled", boolean(&key, &value)?),
            "rcon.port" => ("rcon", "port", integer(&key, &value)?),
            "rcon.password" => ("rcon", "password", Value::String(value)),
            "enable-query" => ("query", "enabled", boolean(&key, &value)?),
            "query.port" => ("query", "port", integer(&key, &value)?),
            "resource-pack" => ("resource_pack", "url", Value::String(value)),
            "resource-pack-sha1" => ("resource_pack", "sha1", Value::String(value)),
            "require-resource-pack" => ("resource_pack", "required", boolean(&key, &value)?),
            "resource-pack-prompt" => ("resource_pack", "prompt", Value::String(value)),
            "resource-pack-id" if value.is_empty() => continue,
            "resource-pack-id" => ("resource_pack", "id", Value::String(value)),
            _ => {
                tracing::warn!(property = %key, "ignoring unsupported server.properties key");
                continue;
            }
        };
        let target = table.entry(section).or_insert_with(|| Value::Table(toml::Table::new()));
        let target = target.as_table_mut().with_context(|| format!("{section} must be a TOML table"))?;
        if !target.contains_key(leaf) {
            target.insert(leaf.into(), value);
        } else {
            tracing::warn!(property = %key, config_key = %format_args!("{section}.{leaf}"), "TOML overrides imported property");
        }
    }
    Ok(())
}

fn integer(key: &str, value: &str) -> anyhow::Result<Value> {
    value.parse().map(Value::Integer).with_context(|| format!("invalid integer for {key}"))
}
fn boolean(key: &str, value: &str) -> anyhow::Result<Value> {
    if value.eq_ignore_ascii_case("true") { return Ok(Value::Boolean(true)); }
    if value.eq_ignore_ascii_case("false") { return Ok(Value::Boolean(false)); }
    anyhow::bail!("invalid boolean for {key}")
}

fn unescape(text: &str) -> anyhow::Result<String> {
    let mut units = Vec::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        let c = if c == '\\' {
            match chars.next().context("unterminated properties escape")? {
                'u' => {
                    let digits: String = chars.by_ref().take(4).collect();
                    anyhow::ensure!(digits.len() == 4, "short properties unicode escape");
                    units.push(u16::from_str_radix(&digits, 16)?);
                    continue;
                }
                'n' => '\n', 'r' => '\r', 't' => '\t', 'f' => '\u{000c}', other => other,
            }
        } else { c };
        units.extend(c.encode_utf16(&mut [0; 2]).iter().copied());
    }
    Ok(String::from_utf16(&units)?)
}

fn parse(text: &str) -> anyhow::Result<std::collections::BTreeMap<String, String>> {
    let mut entries = std::collections::BTreeMap::new();
    let mut logical = String::new();
    let mut continued = false;
    for line in text.lines() {
        let line = line.trim_start_matches([' ', '\t', '\u{000c}']);
        if !continued && (line.is_empty() || line.starts_with(['#', '!'])) { continue; }
        logical.push_str(line);
        if logical.bytes().rev().take_while(|b| *b == b'\\').count() % 2 == 1 {
            logical.pop();
            continued = true;
            continue;
        }
        parse_line(&logical, &mut entries)?;
        logical.clear();
        continued = false;
    }
    if !logical.is_empty() { parse_line(&logical, &mut entries)?; }
    Ok(entries)
}

fn parse_line(line: &str, entries: &mut std::collections::BTreeMap<String, String>) -> anyhow::Result<()> {
    let mut escaped = false;
    let split = line.char_indices().find_map(|(i, c)| {
        if escaped { escaped = false; return None; }
        if c == '\\' { escaped = true; return None; }
        matches!(c, '=' | ':' | ' ' | '\t' | '\u{000c}').then_some(i)
    }).unwrap_or(line.len());
    let mut value = line[split..].trim_start_matches([' ', '\t', '\u{000c}']);
    if value.starts_with(['=', ':']) { value = &value[1..]; }
    let value = value.trim_start_matches([' ', '\t', '\u{000c}']);
    entries.insert(unescape(&line[..split])?, unescape(value)?);
    Ok(())
}

#[cfg(test)]
#[path = "server_properties_tests.rs"]
mod tests;
