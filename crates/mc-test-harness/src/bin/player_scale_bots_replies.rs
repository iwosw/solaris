#[derive(Serialize)]
struct MechanicReply {
    bot: u32,
    window: Option<String>,
    text: String,
}
const MECHANIC_REPLY_CAP: usize = 4096;
const MECHANIC_REPLY_BYTES: usize = 4096;
fn received_literal_text(tag: &mc_nbt::Tag) -> Option<String> {
    fn append(tag: &mc_nbt::Tag, out: &mut String, depth: usize) -> Option<()> {
        use mc_nbt::Tag;
        if depth > 128 {
            return None;
        }
        match tag {
            Tag::String(text) => {
                if out.len().checked_add(text.len())? > MECHANIC_REPLY_BYTES {
                    return None;
                }
                out.push_str(text);
            }
            Tag::List(list) => {
                for child in &list.elements {
                    append(child, out, depth + 1)?;
                }
            }
            Tag::Compound(fields) => {
                if fields.iter().any(|(name, _)| {
                    matches!(
                        name.as_str(),
                        "translate" | "selector" | "score" | "keybind" | "nbt"
                    )
                }) {
                    return None;
                }
                for name in ["text", "extra"] {
                    if let Some((_, child)) = fields.iter().find(|(key, _)| key == name) {
                        append(child, out, depth + 1)?;
                    }
                }
            }
            _ => return None,
        }
        Some(())
    }
    let mut out = String::new();
    append(tag, &mut out, 0)?;
    Some(out)
}

fn harness_chat_echo(text: &str) -> bool {
    let Some((speaker, message)) = text.split_once("> ") else {
        return false;
    };
    if !speaker.starts_with('<') {
        return false;
    }
    if let Some(fields) = message.strip_prefix("scale_open_loop_") {
        let mut parts = fields.split('_');
        return (0..4).all(|_| {
            parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        }) && parts.next().is_none();
    }
    if let Some(fields) = message.strip_prefix("scale_setup_fence_") {
        let mut parts = fields.splitn(3, '_');
        return (0..2).all(|_| {
            parts
                .next()
                .is_some_and(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
        }) && parts.next().is_some_and(|scope| !scope.is_empty());
    }
    false
}

impl Mechanics {
    fn record_reply(&mut self, bot: u32, window: Option<&str>, tag: &mc_nbt::Tag) {
        if self.measurement_epoch.is_some() && window.is_none() {
            return;
        }
        self.command_replies_received += 1;
        match received_literal_text(tag) {
            Some(text) if harness_chat_echo(&text) => self.command_replies_filtered += 1,
            Some(_) if self.command_replies.len() >= MECHANIC_REPLY_CAP => {
                self.command_replies_truncated += 1;
            }
            Some(text) => self.command_replies.push(MechanicReply {
                bot,
                window: window.map(str::to_owned),
                text,
            }),
            None => self.command_replies_unrenderable += 1,
        }
    }
    fn summary(&self, read_only: bool) -> Value {
        json!({"schema_version": 1, "read_only": read_only || self.spec.snapshot_only,
            "baseline_captured": self.measurement_epoch.is_some(),
            "images_omitted": true, "scope": "periodic telemetry only; ready/final/failure carry exact bounded images",
            "cells_observed": self.cells.iter().filter(|cell| cell.initial.is_some()).count(),
            "entities_observed": self.entities.len(),
            "menus_observed": self.menus.iter().filter(|menu| menu.initial.is_some()).count(),
            "received_by_window": self.receive_windows,
            "deaths": self.players.iter().map(|player| player.deaths).sum::<u64>(),
            "respawns": self.players.iter().map(|player| player.respawns).sum::<u64>(),
            "save_completions": self.save_completions,
            "capacity_errors": self.capacity_errors,
            "identity_errors": self.identity_errors, "position_errors": self.position_errors,
            "ghost_updates": self.ghost_updates, "duplicate_spawns": self.duplicate_spawns,
            "duplicate_removals": self.duplicate_removals,
            "command_replies_received": self.command_replies_received,
            "command_replies_truncated": self.command_replies_truncated,
            "command_replies_filtered": self.command_replies_filtered,
            "damage_events_filtered": self.damage_events_filtered,
            "projectile_duplicate_hits": self.projectile_duplicate_hits})
    }
}
