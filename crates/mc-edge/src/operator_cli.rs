//! Offline operator commands, dispatched before content startup or world owners.
use std::path::PathBuf;

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "solaris-v2")]
pub struct Cli {
    /// Server configuration file (TOML).
    #[arg(long, global = true, default_value = "server.toml")]
    pub config: PathBuf,
    /// Disable stdin server commands (useful for services and harnesses).
    #[arg(long)]
    pub no_console: bool,
    /// Bench only: seed cows before accepting connections.
    #[arg(long)]
    pub bench_seed_cows: Option<usize>,
    #[command(subcommand)]
    pub command: Option<Command>,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Derive or validate the operator's vanilla content cache.
    Content {
        #[command(subcommand)]
        command: ContentCommand,
    },
    /// Convert a stopped world without starting the server.
    World {
        #[command(subcommand)]
        command: WorldCommand,
    },
    /// Back up a stopped native world, with a checksum manifest.
    Backup {
        #[arg(long)]
        world_dir: PathBuf,
        #[arg(long)]
        out: PathBuf,
    },
    /// Verify all backup checksums and restore into an empty world directory.
    Restore {
        #[arg(long)]
        from: PathBuf,
        #[arg(long)]
        world_dir: PathBuf,
    },
}

#[derive(Debug, Subcommand)]
pub enum ContentCommand {
    /// Derive a cache from your licensed server bundle; requires JDK 25.
    Import {
        #[arg(long, default_value = mc_protocol::TARGET_RELEASE)]
        version: String,
        #[arg(long, conflicts_with = "download")]
        from: Option<PathBuf>,
        /// Download from Mojang (the default when --from is absent).
        #[arg(long)]
        download: bool,
        #[arg(long)]
        cache: Option<PathBuf>,
    },
    /// Validate the same cache used at startup, without downloading anything.
    Verify {
        #[arg(long)]
        cache: Option<PathBuf>,
    },
}

#[derive(Debug, Subcommand)]
pub enum WorldCommand {
    /// Import modern Anvil into a new native world.
    ImportAnvil(ConversionPaths),
    /// Export a native world and journal tail into a new Anvil world.
    ExportAnvil(ConversionPaths),
    /// Count stored entities, villagers, chunks and settlements of a stopped
    /// native world from its checkpoint, read-only and without starting it.
    Census(CensusPaths),
}

#[derive(Debug, Args)]
pub struct ConversionPaths {
    /// Existing stopped source, never modified.
    #[arg(long)]
    source: PathBuf,
    /// New destination; must not exist.
    #[arg(long)]
    destination: PathBuf,
    #[arg(long)]
    receipt: PathBuf,
}

#[derive(Debug, Args)]
pub struct CensusPaths {
    /// Existing stopped native world, never modified.
    #[arg(long)]
    source: PathBuf,
    /// JSON receipt; must lie outside the source.
    #[arg(long)]
    receipt: PathBuf,
}

/// Backup/restore need only their explicit paths. A fresh content import also
/// works before the operator has created a serving configuration.
pub fn load_config(cli: &Cli) -> anyhow::Result<crate::config::EdgeConfig> {
    if matches!(
        cli.command,
        Some(Command::Backup { .. } | Command::Restore { .. })
    ) {
        return Ok(crate::config::EdgeConfig::default());
    }
    match std::fs::read_to_string(&cli.config) {
        Ok(text) => crate::config::EdgeConfig::from_toml(&text, Some(&cli.config)),
        Err(error)
            if error.kind() == std::io::ErrorKind::NotFound
                && matches!(cli.command, Some(Command::Content { .. })) =>
        {
            Ok(crate::config::EdgeConfig::default())
        }
        Err(error) => Err(error.into()),
    }
}

/// Returns true for a completed offline command, false for normal serving.
pub async fn run_offline(cli: &Cli, cfg: &mut crate::config::EdgeConfig) -> anyhow::Result<bool> {
    let Some(command) = &cli.command else {
        return Ok(false);
    };
    match command {
        Command::Content { command } => {
            let cache = match command {
                ContentCommand::Import { cache, .. } | ContentCommand::Verify { cache } => cache,
            };
            if let Some(cache) = cache {
                cfg.data.vanilla_data_dir = cache.clone();
            }
            let search = crate::content_cache::ContentSearch::for_config(cfg)?;
            match command {
                ContentCommand::Import { version, from, .. } => {
                    let report = crate::content_import::import_content(
                        &crate::content_import::ImportRequest {
                            version: version.clone(),
                            source: from.clone().map_or(
                                crate::content_import::ContentSource::Download,
                                crate::content_import::ContentSource::LocalJar,
                            ),
                            cache: search.import_target(),
                        },
                    )
                    .await?;
                    println!(
                        "content import complete: {} ({} registries, {} entries)",
                        report.cache.display(),
                        report.registries,
                        report.entries
                    );
                }
                ContentCommand::Verify { .. } => {
                    let cache = crate::content_cache::discover_content_cache(&search)?;
                    println!("content cache verified: {}", cache.root().display());
                }
            }
        }
        Command::Backup { world_dir, out } => crate::backup::run_backup(world_dir, out)?,
        Command::Restore { from, world_dir } => crate::backup::run_restore(from, world_dir)?,
        Command::World { command } => {
            crate::server::resolve_content_config(cfg)?;
            let config = crate::server::core_config(cfg);
            match command {
                WorldCommand::ImportAnvil(paths) => {
                    mc_core::world_conversion::import_anvil(
                        &config,
                        &paths.source,
                        &paths.destination,
                        &paths.receipt,
                    )?;
                }
                WorldCommand::ExportAnvil(paths) => {
                    mc_core::world_conversion::export_anvil(
                        &config,
                        &paths.source,
                        &paths.destination,
                        &paths.receipt,
                    )?;
                }
                WorldCommand::Census(paths) => {
                    mc_core::world_conversion::census_native(
                        &config,
                        &paths.source,
                        &paths.receipt,
                    )?;
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "operator_cli_tests.rs"]
mod tests;
