//! Optional remote operator services. Dropping the owner aborts all sockets.
use std::sync::Arc;
use crate::server::EdgeServer;

#[path = "rcon.rs"]
mod rcon;
#[path = "query.rs"]
mod query;

pub(crate) async fn start(server: Arc<EdgeServer>) -> anyhow::Result<tokio::task::JoinSet<()>> {
    let mut tasks = tokio::task::JoinSet::new();
    // Bind before serving: requested services must not silently fail startup.
    let rcon = if server.cfg.rcon.enabled {
        anyhow::ensure!(!server.cfg.rcon.password.is_empty(), "RCON requires a password");
        Some(tokio::net::TcpListener::bind((server.cfg.rcon.bind_address.as_str(), server.cfg.rcon.port)).await?)
    } else { None };
    let query = if server.cfg.query.enabled {
        Some(tokio::net::UdpSocket::bind((server.cfg.network.bind_address.as_str(), server.cfg.query.port)).await?)
    } else { None };
    if let Some(listener) = rcon {
        let server = Arc::clone(&server);
        tasks.spawn(async move {
            if let Err(error) = rcon::serve(listener, server).await { tracing::error!(%error, "RCON stopped"); }
        });
    }
    if let Some(socket) = query {
        tasks.spawn(async move {
            if let Err(error) = query::serve(socket, server).await { tracing::error!(%error, "Query stopped"); }
        });
    }
    Ok(tasks)
}
