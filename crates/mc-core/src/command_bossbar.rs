//! Vanilla `/bossbar` grammar (BossBarCommands.java): parse only.
//! Execution lives in [`crate::bossbar`].

use crate::bossbar::{BarColor, BarOverlay};
use crate::commands::CommandError;

/// Typed `/bossbar` command. Component payloads stay raw text here; the
/// executor parses/resolves them via `crate::command_text`.
#[derive(Debug, Clone, PartialEq)]
pub enum BossBarCommand {
    Add {
        id: String,
        name: String,
    },
    Remove {
        id: String,
    },
    List,
    SetName {
        id: String,
        name: String,
    },
    SetColor {
        id: String,
        color: BarColor,
    },
    SetStyle {
        id: String,
        overlay: BarOverlay,
    },
    SetValue {
        id: String,
        value: i32,
    },
    SetMax {
        id: String,
        max: i32,
    },
    SetVisible {
        id: String,
        visible: bool,
    },
    /// `targets: None` clears the player list (`set ... players` bare).
    SetPlayers {
        id: String,
        targets: Option<String>,
    },
    GetValue {
        id: String,
    },
    GetMax {
        id: String,
    },
    GetVisible {
        id: String,
    },
    GetPlayers {
        id: String,
    },
}

pub const USAGE: &str = "bossbar <add|remove|list|set|get> ...";

fn bar_id(word: &str) -> Result<String, CommandError> {
    match mc_data::Identifier::parse(word) {
        Ok(id) => Ok(id.as_str().to_owned()),
        Err(_) => Err(CommandError::Message(format!(
            "Invalid id '{word}': expected minecraft:bar_name"
        ))),
    }
}

fn integer(word: &str, minimum: i32) -> Result<i32, CommandError> {
    let value: i32 = word
        .parse()
        .map_err(|_| CommandError::Message(format!("Expected integer, got '{word}'")))?;
    if value < minimum {
        return Err(CommandError::Message(format!(
            "Expected integer >= {minimum}, got '{word}'"
        )));
    }
    Ok(value)
}

fn boolean(word: &str) -> Result<bool, CommandError> {
    match word {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(CommandError::Message(format!(
            "Expected true or false, got '{word}'"
        ))),
    }
}

/// Parse `args` after the `bossbar` root. `command` is the full command text
/// (for the multi-word component tail of `add`/`set name`).
pub fn parse(args: &[&str], command: &str) -> Result<BossBarCommand, CommandError> {
    let (verb, rest) = args.split_first().ok_or(CommandError::Usage(USAGE))?;
    match *verb {
        "add" => {
            let [id, ..] = rest else {
                return Err(CommandError::Usage(USAGE));
            };
            let id = bar_id(id)?;
            // Component is the remainder of the raw command after the id word.
            let tail = tail_after(command, &["bossbar", "add", rest[0]])?;
            crate::command_text::parse_component(&tail)?;
            Ok(BossBarCommand::Add { id, name: tail })
        }
        "remove" => match rest {
            [id] => Ok(BossBarCommand::Remove { id: bar_id(id)? }),
            _ => Err(CommandError::Usage(USAGE)),
        },
        "list" => {
            if rest.is_empty() {
                Ok(BossBarCommand::List)
            } else {
                Err(CommandError::Usage(USAGE))
            }
        }
        "set" => {
            let [id, field, ..] = rest else {
                return Err(CommandError::Usage(USAGE));
            };
            let id = bar_id(id)?;
            match *field {
                "name" => {
                    let tail = tail_after(command, &["bossbar", "set", rest[0], "name"])?;
                    crate::command_text::parse_component(&tail)?;
                    Ok(BossBarCommand::SetName { id, name: tail })
                }
                "color" => match &rest[2..] {
                    [color] => match BarColor::parse(color) {
                        Some(color) => Ok(BossBarCommand::SetColor { id, color }),
                        None => Err(CommandError::Message(format!(
                            "Invalid color '{color}': expected pink|blue|red|green|yellow|purple|white"
                        ))),
                    },
                    _ => Err(CommandError::Usage(USAGE)),
                },
                "style" => match &rest[2..] {
                    [style] => match BarOverlay::parse(style) {
                        Some(overlay) => Ok(BossBarCommand::SetStyle { id, overlay }),
                        None => Err(CommandError::Message(format!(
                            "Invalid style '{style}': expected progress|notched_6|notched_10|notched_12|notched_20"
                        ))),
                    },
                    _ => Err(CommandError::Usage(USAGE)),
                },
                "value" => match &rest[2..] {
                    [value] => Ok(BossBarCommand::SetValue {
                        id,
                        value: integer(value, 0)?,
                    }),
                    _ => Err(CommandError::Usage(USAGE)),
                },
                "max" => match &rest[2..] {
                    [max] => Ok(BossBarCommand::SetMax {
                        id,
                        max: integer(max, 1)?,
                    }),
                    _ => Err(CommandError::Usage(USAGE)),
                },
                "visible" => match &rest[2..] {
                    [visible] => Ok(BossBarCommand::SetVisible {
                        id,
                        visible: boolean(visible)?,
                    }),
                    _ => Err(CommandError::Usage(USAGE)),
                },
                "players" => match &rest[2..] {
                    [] => Ok(BossBarCommand::SetPlayers { id, targets: None }),
                    [targets] => Ok(BossBarCommand::SetPlayers {
                        id,
                        targets: Some((*targets).to_owned()),
                    }),
                    _ => Err(CommandError::Usage(USAGE)),
                },
                _ => Err(CommandError::Usage(USAGE)),
            }
        }
        "get" => match rest {
            [id, field] => {
                let id = bar_id(id)?;
                match *field {
                    "value" => Ok(BossBarCommand::GetValue { id }),
                    "max" => Ok(BossBarCommand::GetMax { id }),
                    "visible" => Ok(BossBarCommand::GetVisible { id }),
                    "players" => Ok(BossBarCommand::GetPlayers { id }),
                    _ => Err(CommandError::Usage(USAGE)),
                }
            }
            _ => Err(CommandError::Usage(USAGE)),
        },
        _ => Err(CommandError::Usage(USAGE)),
    }
}

/// Raw-text tail after the given leading words (whitespace-separated prefix).
/// `command_words` keeps balanced `{...}`/`[...]`/quoted text as one word,
/// so the component JSON survives intact here.
fn tail_after(command: &str, leading: &[&str]) -> Result<String, CommandError> {
    let mut rest = command.trim_start();
    // Strip the leading slash the edge may keep (`/bossbar ...`).
    rest = rest.strip_prefix('/').unwrap_or(rest);
    for word in leading {
        let stripped = rest.strip_prefix(word).ok_or(CommandError::Usage(USAGE))?;
        if !stripped.is_empty() && !stripped.starts_with(char::is_whitespace) {
            return Err(CommandError::Usage(USAGE));
        }
        rest = stripped.trim_start();
        if rest.is_empty() {
            break;
        }
    }
    if rest.is_empty() {
        return Err(CommandError::Usage(USAGE));
    }
    Ok(rest.to_owned())
}

#[cfg(test)]
#[path = "command_bossbar_tests.rs"]
mod tests;
