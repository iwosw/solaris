//! Line-oriented operator console. A detached stdin reader cannot hold Tokio's
//! blocking pool open after stop/SIGTERM while waiting for another input line.
use std::io::{BufRead, Write};
use std::sync::Arc;

use mc_core::boundary::{PlayerChange, PlayerInput};
use mc_core::command_console::{CONNECTION, ConsoleCommand};

use crate::server::EdgeServer;

pub(crate) fn spawn(server: Arc<EdgeServer>) -> std::io::Result<tokio::task::JoinHandle<()>> {
    let (sender, mut lines) = tokio::sync::mpsc::channel(16);
    std::thread::Builder::new()
        .name("server-console".into())
        .spawn(move || {
            for line in std::io::stdin().lock().lines() {
                let failed = line.is_err();
                if sender.blocking_send(line).is_err() || failed {
                    break;
                }
            }
        })?;
    Ok(tokio::spawn(async move {
        while let Some(line) = lines.recv().await {
            match line {
                Ok(line) => {
                    if let Err(error) = execute(&server, &line).await {
                        print_line(&format!("Command failed: {error:#}"));
                    }
                }
                Err(error) => {
                    print_line(&format!("Console input failed: {error}"));
                    break;
                }
            }
        }
    }))
}

/// Console-only `backup`: copies the running world into `[data] backup_dir`
/// without pausing ticks. It runs detached, so the console stays usable and
/// the outcome prints when the copy finishes.
fn backup(server: &EdgeServer, command: &str) {
    if command != "backup" {
        print_line("Incorrect argument for command");
        return;
    }
    let Some(directory) = server.cfg.data.backup_dir.clone() else {
        print_line("Backups are disabled: set backup_dir under [data]");
        return;
    };
    print_line("Backing up the world (the server keeps running)");
    let core = Arc::clone(&server.hub);
    tokio::spawn(async move {
        let started = std::time::Instant::now();
        let result =
            tokio::task::spawn_blocking(move || crate::backup::run_hot_backup(&core, &directory))
                .await;
        match result {
            Ok(Ok((path, backup))) => print_line(&format!(
                "Backup {} complete: checkpoint tick {}, recoverable to tick {}, {} files, {} ms",
                path.display(),
                backup.checkpoint_tick,
                backup.wal_tick,
                backup.files.len(),
                started.elapsed().as_millis()
            )),
            Ok(Err(error)) => print_line(&format!("Backup failed: {error:#}")),
            Err(error) => print_line(&format!("Backup failed: {error}")),
        }
    });
}

pub(crate) async fn execute(server: &EdgeServer, line: &str) -> anyhow::Result<()> {
    let command = line.trim().trim_start_matches('/');
    if command.is_empty() {
        return Ok(());
    }
    if command.split_whitespace().next() == Some("backup")
        && !server
            .hub
            .plugin_commands()
            .iter()
            .any(|(name, _)| name == "backup")
    {
        backup(server, command);
        return Ok(());
    }
    if server.handle_admin_command(CONNECTION, command).await? {
        return Ok(());
    }
    let mut published = server.hub.subscribe();
    let (command, completed) = ConsoleCommand::new(command.into());
    let input = PlayerInput::ConsoleCommand(command);
    // Console input uses the same bounded admission as authenticated players.
    anyhow::ensure!(
        server.hub.submit(input),
        "authoritative command queue is full"
    );
    let completed_tick = completed
        .await
        .map_err(|_| anyhow::anyhow!("core stopped before command execution"))?;
    loop {
        match published.recv().await {
            Ok(output) if output.tick >= completed_tick => return Ok(()),
            Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                anyhow::bail!("core stopped before command publication");
            }
        }
    }
}

pub(crate) fn print_line(message: &str) {
    let mut stdout = std::io::stdout().lock();
    let _ = writeln!(stdout, "{message}");
    let _ = stdout.flush();
}

fn component_text(component: &mc_nbt::Tag) -> String {
    use mc_nbt::Tag;
    match component {
        Tag::String(text) => text.clone(),
        Tag::List(list) => list.elements.iter().map(component_text).collect(),
        Tag::Compound(fields) => {
            let field = |key: &str| {
                fields
                    .iter()
                    .find(|(name, _)| name == key)
                    .map(|(_, value)| value)
            };
            let mut text = field("text").map(component_text).unwrap_or_else(|| {
                let mut text = field("fallback")
                    .or_else(|| field("translate"))
                    .map(component_text)
                    .unwrap_or_default();
                if let Some(Tag::List(arguments)) = field("with") {
                    text.push_str(": ");
                    text.push_str(
                        &arguments
                            .elements
                            .iter()
                            .map(component_text)
                            .collect::<Vec<_>>()
                            .join(", "),
                    );
                }
                text
            });
            if let Some(extra) = field("extra") {
                text.push_str(&component_text(extra));
            }
            text
        }
        _ => String::new(),
    }
}

pub(crate) fn output(changes: &[PlayerChange]) {
    for change in changes {
        match change {
            PlayerChange::Chat { message, .. } => print_line(message),
            PlayerChange::StyledChat { content, .. } => print_line(&component_text(content)),
            PlayerChange::Disconnect { reason, .. } => print_line(reason),
            PlayerChange::Title(
                mc_core::boundary::TitleChange::Title(content)
                | mc_core::boundary::TitleChange::Subtitle(content)
                | mc_core::boundary::TitleChange::ActionBar(content),
            ) => print_line(&component_text(content)),
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "console_tests.rs"]
mod tests;
