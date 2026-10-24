//! `/datapack` management: enable/disable ordering, listing with hover,
//! atomic activation.
//!
//! Vanilla boundary (Main-approved): `enable`/`disable` swap compiled
//! functions/tags live via the `/reload` worker. Supported recipe/block-loot
//! changes refuse atomically with `restart-required`; worldgen/dimensions/
//! other registries/ZIP refuse atomically as unsupported (never a false
//! restart promise). Listing serves the cached compiled catalog refreshed by
//! the worker; immediate filesystem discovery is the open
//! DATAPACK-LIST-LIVE-DISCOVERY residual. Hot reload of recipes/loot
//! (DATAPACK-HOT-RELOAD-RECIPES-LOOT), archives and worldgen
//! (DATAPACK-WORLDGEN-ZIP), and feature-gated admission
//! (DATAPACK-FEATURE-ADMISSION) stay open. No tick-thread filesystem I/O:
//! all loads run on the reload worker thread.
use super::{Selection, feedback};
use crate::tick::TickState;

fn quoted(name: &str) -> String {
    let mut out = String::with_capacity(name.len() + 2);
    out.push('"');
    for ch in name.chars() {
        if matches!(ch, '"' | '\\') {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    out
}

fn unquote(word: &str) -> Option<String> {
    let body = word.strip_prefix('"')?.strip_suffix('"')?;
    if body.is_empty() {
        return None;
    }
    let mut out = String::with_capacity(body.len());
    let mut chars = body.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            out.push(chars.next()?);
        } else if ch == '"' {
            return None;
        } else {
            out.push(ch);
        }
    }
    Some(out)
}

fn pack_name(word: &str) -> Option<String> {
    if word.starts_with('"') {
        return unquote(word);
    }
    if word.is_empty()
        || word.len() > 1024
        || word.contains([' ', '\t', '\n', '[', ']', '{', '}'])
    {
        return None;
    }
    Some(word.to_owned())
}

/// Parse a pack reference: `available|enabled <name>`, or a bare/quotable name.
fn reference(words: &[&str]) -> Option<String> {
    match words {
        ["available", name] | ["enabled", name] => pack_name(name),
        [name] => pack_name(name),
        _ => None,
    }
}

pub(crate) fn command(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    text: &str,
) -> crate::command_execution::CommandResult {
    use crate::command_execution::CommandResult::{Failed, Success, Unavailable};
    if crate::command_context::exhausted(state) {
        return Unavailable;
    }
    let words: Vec<&str> = crate::commands::command_words(text)
        .map(|words| words.into_iter().collect())
        .unwrap_or_default();
    let result = dispatch(state, connection, words.as_slice());
    match result {
        Ok(count) => Success(count),
        Err(message) => {
            feedback(state, connection, message);
            Failed
        }
    }
}

fn dispatch(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    words: &[&str],
) -> Result<i32, String> {
    match words {
        ["datapack", "list"] | ["datapack", "list", "available" | "enabled"] => {
            list(state, connection, words)
        }
        ["datapack", "enable", rest @ ..] => enable(state, rest),
        ["datapack", "disable", rest @ ..] => disable(state, rest),
        ["datapack", "create", ..] => Err(
            "Datapack creation is not supported on this server".into(),
        ),
        _ => Err(
            "Supported: datapack list [available|enabled]; datapack enable <pack> [first|last|before|after <pack>]; datapack disable <pack>".into(),
        ),
    }
}

/// One listed pack as a hoverable chat component: the quoted name shows the
/// `pack.mcmeta` description on hover (DATAPACK-LIST-HOVER).
fn hover_entry(name: &str, description: &str) -> mc_nbt::Tag {
    let mut fields = vec![("text".to_owned(), mc_nbt::Tag::String(quoted(name)))];
    if !description.is_empty() {
        fields.push((
            "hoverEvent".to_owned(),
            mc_nbt::Tag::Compound(vec![
                (
                    "action".to_owned(),
                    mc_nbt::Tag::String("show_text".to_owned()),
                ),
                (
                    "contents".to_owned(),
                    mc_nbt::Tag::String(description.to_owned()),
                ),
            ]),
        ));
    }
    mc_nbt::Tag::Compound(fields)
}

fn list(
    state: &mut TickState,
    connection: crate::boundary::ConnectionId,
    words: &[&str],
) -> Result<i32, String> {
    // Cached compiled catalog: the exact snapshot counts below are the
    // execute-store results. The reload worker refreshes this snapshot;
    // listing never touches the filesystem itself.
    let packs = state.datapacks.packs().clone();
    let selection = packs.selection.clone();
    let show_available = !matches!(words, ["datapack", "list", "enabled"]);
    let show_enabled = !matches!(words, ["datapack", "list", "available"]);
    let mut entries = Vec::new();
    if show_available {
        for name in &selection.disabled {
            let description = packs.descriptions.get(name).map(String::as_str).unwrap_or("");
            entries.push(hover_entry(name, description));
        }
    }
    if show_enabled {
        for name in &selection.enabled {
            let description = packs.descriptions.get(name).map(String::as_str).unwrap_or("");
            entries.push(hover_entry(name, description));
        }
    }
    let count = match words {
        ["datapack", "list", "available"] => selection.disabled.len(),
        ["datapack", "list", "enabled"] | ["datapack", "list"] => selection.enabled.len(),
        _ => selection.enabled.len() + selection.disabled.len(),
    };
    let label = match words {
        ["datapack", "list", "available"] => "Available",
        ["datapack", "list", "enabled"] => "Enabled",
        _ => "Enabled",
    };
    crate::command_execution::feedback(
        state,
        connection,
        "commands.datapack.list",
        vec![
            mc_nbt::Tag::String(label.to_owned()),
            mc_nbt::Tag::String(count.to_string()),
            mc_nbt::Tag::List(mc_nbt::ListTag {
                element_type: mc_nbt::tag_type::COMPOUND,
                elements: entries,
            }),
        ],
    );
    Ok(count as i32)
}

fn move_enabled(selection: &mut Selection, id: &str, rest: &[&str]) -> Result<(), String> {
    let position = selection
        .enabled
        .iter()
        .position(|entry| entry == id)
        .ok_or_else(|| format!("Datapack {id} is not enabled"))?;
    match rest {
        [] | ["last"] => {
            let entry = selection.enabled.remove(position);
            selection.enabled.push(entry);
        }
        ["first"] => {
            let entry = selection.enabled.remove(position);
            // `vanilla` stays first: enabling past it is unsupported.
            let at = if selection.enabled.first().is_some_and(|first| first == "vanilla") {
                1
            } else {
                0
            };
            selection.enabled.insert(at.min(selection.enabled.len()), entry);
        }
        ["before", other] | ["after", other] => {
            let other = pack_name(other).ok_or_else(|| format!("Unknown datapack {other}"))?;
            let entry = selection.enabled.remove(position);
            let at = selection
                .enabled
                .iter()
                .position(|entry| entry == &other)
                .ok_or_else(|| format!("Unknown datapack {other}"))?;
            let at = if rest[0] == "after" { at + 1 } else { at };
            // Never order before the builtin vanilla pack.
            let at = if selection.enabled.first().is_some_and(|first| first == "vanilla") {
                at.max(1)
            } else {
                at
            };
            selection.enabled.insert(at.min(selection.enabled.len()), entry);
        }
        _ => return Err("Supported: datapack enable <pack> [first|last|before|after <pack>]".into()),
    }
    Ok(())
}

fn enable(state: &mut TickState, rest: &[&str]) -> Result<i32, String> {
    let (name, order) = match rest {
        [name, order @ ..] => (reference(&[*name]).ok_or_else(|| format!("Unknown datapack {name}"))?, order),
        [] => return Err("Supported: datapack enable <pack> [first|last|before|after <pack>]".into()),
    };
    if name == "vanilla" {
        return Err("Datapack vanilla is already enabled".into());
    }
    let mut selection = state.datapacks.selection().clone();
    if selection.enabled.iter().any(|entry| entry == &name) {
        if order.is_empty() {
            return Err(format!("Datapack {name} is already enabled"));
        }
        move_enabled(&mut selection, &name, order)?;
    } else {
        if !selection.disabled.iter().any(|entry| entry == &name) {
            return Err(format!("Unknown datapack {name}"));
        }
        selection.disabled.retain(|entry| entry != &name);
        selection.enabled.push(name.clone());
        if !order.is_empty() {
            move_enabled(&mut selection, &name, order)?;
        }
    }
    activate(state, selection)
}

fn disable(state: &mut TickState, rest: &[&str]) -> Result<i32, String> {
    let name = match rest {
        [name] => reference(&[name]).ok_or_else(|| format!("Unknown datapack {name}"))?,
        _ => return Err("Supported: datapack disable <pack>".into()),
    };
    if name == "vanilla" {
        return Err("Disabling builtin vanilla content is unsupported".into());
    }
    let mut selection = state.datapacks.selection().clone();
    if !selection.enabled.iter().any(|entry| entry == &name) {
        if selection.disabled.iter().any(|entry| entry == &name) {
            return Err(format!("Datapack {name} is already disabled"));
        }
        return Err(format!("Unknown datapack {name}"));
    }
    selection.enabled.retain(|entry| entry != &name);
    selection.disabled.push(name);
    activate(state, selection)
}

/// Atomically activate `selection` through the reload worker: validate, queue
/// a worker recompile, and return the loaded-pack count. The worker classifies
/// the outcome per the approved boundary on completion — functions/tags swap
/// live, supported recipe/block-loot diffs refuse with restart-required,
/// worldgen/dimensions/other registries refuse as unsupported — while the old
/// packs and selection stay authoritative until the worker reports back.
fn activate(state: &mut TickState, selection: Selection) -> Result<i32, String> {
    selection.validate()?;
    let count = selection.enabled.len() as i32;
    state.datapacks.request_activation(selection)?;
    Ok(count)
}
