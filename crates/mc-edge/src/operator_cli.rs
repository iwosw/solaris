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
    /// Derive a cache from your licensed server bundles; requires JDK 25.
    Import {
        #[arg(long, default_value = mc_protocol::TARGET_RELEASE)]
        version: String,
        /// Server bundle of `--version` (protocol identity and reports).
        #[arg(long, conflicts_with = "download")]
        from: Option<PathBuf>,
        /// Server bundle of the gameplay data-pack release (26.1.2).
        #[arg(long, conflicts_with = "download")]
        gameplay_from: Option<PathBuf>,
        /// Download from Mojang (the default for each bundle not supplied).
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
    /// Read exact blocks, typed container contents, stored player inventories and
    /// stored item entities of a stopped native world, read-only and without
    /// starting it. The receipt must lie outside the source.
    Probe(ProbePaths),
    /// Keep a stopped native world across a worldgen revision bump: rewrite its
    /// identity and generation records to the current revision. Existing chunks
    /// stay as generated; only not-yet-generated chunks use the new generator.
    /// Refuses when anything but the revision differs (seed comes from --config).
    UpgradeWorldgen,
    /// Generate missing overworld chunks offline and store them, so joining
    /// players load them instead of waiting for worldgen. Holds the world
    /// lease; rerunning skips chunks already stored.
    Pregenerate {
        /// Radius in blocks around the center (both axes).
        #[arg(long, value_parser = parse_block_radius)]
        radius: u32,
        /// Block center `x,z`; defaults to the world spawn.
        #[arg(long, value_parser = parse_block_center, allow_hyphen_values = true)]
        center: Option<[i32; 2]>,
    },
    /// Forget overworld chunks outside a keep radius that the generator
    /// created and nobody changed, lived in or left entities in, then compact
    /// the file. Anvil-imported chunks are never pruned. A pruned chunk
    /// regenerates with the current generator when next visited. Holds the world lease.
    Prune {
        /// Chunks within this many blocks of the center are never pruned.
        #[arg(long)]
        keep_radius: u32,
        /// Block center `x,z`; defaults to the world spawn.
        #[arg(long, value_parser = parse_block_center, allow_hyphen_values = true)]
        center: Option<[i32; 2]>,
        /// Prune only chunks inhabited for at most this many ticks.
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(i64).range(0..))]
        max_inhabited_ticks: i64,
        /// Report what would be pruned and write nothing.
        #[arg(long)]
        dry_run: bool,
    },
    /// Rewrite chunk rows stored before compression and return free pages to
    /// the file system, shrinking `native/state.redb`. Holds the world lease.
    Compact,
}

/// Chunk pregeneration takes a block radius; zero would do nothing.
fn parse_block_radius(text: &str) -> Result<u32, String> {
    text.parse::<u32>()
        .ok()
        .filter(|radius| *radius > 0)
        .ok_or_else(|| format!("radius must be a positive block count, got {text}"))
}

fn parse_block_center(text: &str) -> Result<[i32; 2], String> {
    let (x, z) = text
        .split_once(',')
        .ok_or_else(|| format!("center must be `x,z` block coordinates, got {text}"))?;
    let parse = |value: &str, axis: char| {
        value
            .trim()
            .parse::<i32>()
            .map_err(|_| format!("center {axis} must be an integer block coordinate, got {value}"))
    };
    Ok([parse(x, 'x')?, parse(z, 'z')?])
}

fn parse_probe_block(text: &str) -> Result<[i32; 3], String> {
    let mut parts = text.split(',');
    let mut next = |axis: char| -> Result<i32, String> {
        parts
            .next()
            .ok_or_else(|| format!("block must be `x,y,z` coordinates, got {text}"))?
            .trim()
            .parse::<i32>()
            .map_err(|_| format!("block {axis} must be an integer block coordinate, got {text}"))
    };
    let x = next('x')?;
    let y = next('y')?;
    let z = next('z')?;
    if parts.next().is_some() {
        return Err(format!("block must be `x,y,z` coordinates, got {text}"));
    }
    Ok([x, y, z])
}

#[derive(Debug, Args)]
pub struct ProbePaths {
    /// Existing stopped native world, never modified.
    #[arg(long)]
    source: PathBuf,
    /// JSON receipt; must lie outside the source.
    #[arg(long)]
    receipt: PathBuf,
    /// Block to read as `x,y,z` (overworld). Repeatable.
    #[arg(
        long = "block",
        value_parser = parse_probe_block,
        required = true,
        allow_hyphen_values = true
    )]
    blocks: Vec<[i32; 3]>,
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
        Ok(text) => crate::server_properties::load(&text, &cli.config),
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
                ContentCommand::Import {
                    version,
                    from,
                    gameplay_from,
                    ..
                } => {
                    let source = |jar: &Option<PathBuf>| {
                        jar.clone().map_or(
                            crate::content_import::ContentSource::Download,
                            crate::content_import::ContentSource::LocalJar,
                        )
                    };
                    let report = crate::content_import::import_content(
                        &crate::content_import::ImportRequest {
                            version: version.clone(),
                            source: source(from),
                            gameplay_source: source(gameplay_from),
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
                WorldCommand::Probe(paths) => {
                    mc_core::world_conversion::probe_native(
                        &config,
                        &paths.source,
                        &paths.blocks,
                        &paths.receipt,
                    )?;
                }
                WorldCommand::UpgradeWorldgen => {
                    let report = mc_core::worldgen_upgrade::upgrade_worldgen_revision(&config)?;
                    if report.rewritten {
                        println!(
                            "worldgen revision upgraded: {} -> {} (existing chunks unchanged)",
                            report.from_revision, report.to_revision
                        );
                    } else {
                        println!(
                            "worldgen revision already current: {}; nothing changed",
                            report.to_revision
                        );
                    }
                }
                WorldCommand::Pregenerate { radius, center } => {
                    let request = mc_core::pregenerate::PregenerateRequest {
                        radius: *radius,
                        center: *center,
                    };
                    // Generation is synchronous and its worker pool blocks for
                    // results, so it runs on the blocking pool, never on an
                    // async worker.
                    let report = tokio::task::spawn_blocking(move || {
                        mc_core::pregenerate::pregenerate(&config, &request, &mut |progress| {
                            let done = progress.already_stored + progress.generated;
                            let percent = if progress.chunks == 0 {
                                100.0
                            } else {
                                done as f64 * 100.0 / progress.chunks as f64
                            };
                            println!(
                                "pregenerate: {done}/{} chunks ({percent:.1}%), {} generated, \
                                 {} already stored, {:.1}s",
                                progress.chunks,
                                progress.generated,
                                progress.already_stored,
                                progress.elapsed_ms as f64 / 1000.0,
                            );
                        })
                    })
                    .await
                    .map_err(|error| anyhow::anyhow!("pregenerate task failed: {error}"))??;
                    println!(
                        "pregenerate complete: chunks [{},{}]..[{},{}] around block ({},{}), \
                         {} chunks, {} generated, {} already stored, {:.1}s",
                        report.chunk_min[0],
                        report.chunk_min[1],
                        report.chunk_max[0],
                        report.chunk_max[1],
                        report.center[0],
                        report.center[1],
                        report.chunks,
                        report.generated,
                        report.already_stored,
                        report.elapsed_ms as f64 / 1000.0,
                    );
                }
                WorldCommand::Prune {
                    keep_radius,
                    center,
                    max_inhabited_ticks,
                    dry_run,
                } => {
                    let report = mc_core::world_maintenance::prune_world(
                        &config,
                        &mc_core::world_maintenance::PruneRequest {
                            keep_radius: *keep_radius,
                            center: *center,
                            max_inhabited_ticks: *max_inhabited_ticks,
                            dry_run: *dry_run,
                        },
                    )?;
                    let chunks = report.chunks;
                    println!(
                        "prune{}: kept chunks [{},{}]..[{},{}] around block ({},{}); \
                         {} stored, {} pruned, {} kept inside the radius, {} kept inhabited, \
                         {} kept changed, {} kept not generator-made (imported or pre-provenance); \
                         file {} -> {} bytes",
                        if *dry_run { " (dry run)" } else { "" },
                        report.keep_chunk_min[0],
                        report.keep_chunk_min[1],
                        report.keep_chunk_max[0],
                        report.keep_chunk_max[1],
                        report.center[0],
                        report.center[1],
                        chunks.examined,
                        chunks.pruned,
                        chunks.kept_inside_rectangle,
                        chunks.kept_inhabited,
                        chunks.kept_changed,
                        chunks.kept_not_generated,
                        report.bytes_before,
                        report.bytes_after,
                    );
                }
                WorldCommand::Compact => {
                    let report = mc_core::world_maintenance::compact_world(&config)?;
                    println!(
                        "compact complete: {} rows repacked; file {} -> {} bytes",
                        report.rewritten_rows, report.bytes_before, report.bytes_after,
                    );
                }
            }
        }
    }
    Ok(true)
}

#[cfg(test)]
#[path = "operator_cli_tests.rs"]
mod tests;
