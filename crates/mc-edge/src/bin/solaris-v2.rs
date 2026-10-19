//! `solaris-v2`: the v2 edge server binary.
//!
//! Serves with `--config PATH`, with stdin commands unless `--no-console`.
//! Offline content, backup/restore and Anvil conversion commands run before
//! starting core owners or a listener. Signals and console `stop` request a
//! clean shutdown and final native checkpoint.

use clap::Parser;
use mc_edge::operator_cli::Cli;
use tracing::info;

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();
    let cli = Cli::parse();
    let cfg = mc_edge::operator_cli::load_config(&cli)?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        // Chunk frames are encoded on runtime workers; keep bounded stack headroom.
        .thread_stack_size(16 * 1024 * 1024)
        .build()?
        .block_on(run(cli, cfg))
}

async fn run(cli: Cli, mut cfg: mc_edge::config::EdgeConfig) -> anyhow::Result<()> {
    if mc_edge::operator_cli::run_offline(&cli, &mut cfg).await? {
        return Ok(());
    }
    mc_edge::server::resolve_content_config(&mut cfg)?;
    let data = mc_edge::server::load_vanilla_data(&cfg)?;
    let tags = mc_edge::server::load_tags(&cfg, &data)?;
    let core_config = mc_edge::server::core_config(&cfg);
    // Registered before the core opens: a stop requested during startup must
    // still reach the clean shutdown below instead of the default signal kill.
    let shutdown = shutdown_signal();
    // Recovery can wait for chunk workers while folding durable block overlays.
    let core = std::sync::Arc::new(
        tokio::task::spawn_blocking(move || mc_core::Core::open(core_config)).await??,
    );
    if let Some(cows) = cli.bench_seed_cows {
        let seed_core = std::sync::Arc::clone(&core);
        tokio::task::spawn_blocking(move || mc_core::bench_seed(&seed_core, cows, true, 0)).await?;
        let [spawn_x, spawn_y, spawn_z] = core.world_spawn();
        info!(cows, spawn_x, spawn_y, spawn_z, "bench seed spawned");
    }
    let shutdown_core = std::sync::Arc::clone(&core);
    let dashboard = cfg.dashboard.enabled.then(|| {
        (
            cfg.dashboard.bind_address.clone(),
            cfg.dashboard.port,
            mc_edge::server::dashboard_allowed(&cfg),
        )
    });

    let listener = tokio::net::TcpListener::bind(cfg.bind_addr()).await?;
    info!(
        addr = %listener.local_addr()?,
        name = %cfg.server.name,
        "Solaris is listening (edge v2)"
    );
    let server = mc_edge::server::EdgeServer::new(core, data, tags, cfg)?;
    // Serve the state the tick pump updates (owned by the server).
    if let Some((addr, port, allowed)) = dashboard {
        if allowed {
            info!(addr = %addr, port, "dashboard listening");
            let state = std::sync::Arc::clone(&server.dashboard);
            tokio::spawn(mc_edge::dashboard::serve(addr, port, state));
        } else {
            tracing::warn!(
                addr = %addr,
                "dashboard enabled with non-loopback bind but allow_remote=false; dashboard not started"
            );
        }
    }
    tokio::select! {
        biased;
        result = mc_edge::server::serve_with_console(listener, server, !cli.no_console) => {
            // The edge stopped (pump death or accept failure): stop the core
            // cleanly too instead of leaving it ticking unobserved.
            shutdown_core.shutdown();
            result
        }
        _ = shutdown => {
            shutdown_core.shutdown();
            info!("shutdown requested; core stopped");
            Ok(())
        }
    }
}

/// Ctrl-C or SIGTERM: either requests shutdown, then the process exits.
/// Handlers are installed when this is called, not when the future is polled.
fn shutdown_signal() -> impl Future<Output = ()> {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let streams = signal(SignalKind::terminate())
            .and_then(|term| signal(SignalKind::interrupt()).map(|interrupt| (term, interrupt)));
        async move {
            match streams {
                Ok((mut term, mut interrupt)) => {
                    tokio::select! {
                        _ = term.recv() => {},
                        _ = interrupt.recv() => {},
                    }
                }
                Err(_) => {
                    let _ = tokio::signal::ctrl_c().await;
                }
            }
        }
    }
    #[cfg(not(unix))]
    async {
        let _ = tokio::signal::ctrl_c().await;
    }
}
