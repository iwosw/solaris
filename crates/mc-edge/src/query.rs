//! GameSpy4 wire from local 26.1.2 javap QueryThreadGs4/NetworkDataOutputStream.
//! Stateless source-bound challenges avoid an unauthenticated allocation table.
use crate::server::EdgeServer;
use sha2::{Digest, Sha256};
use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

struct Query {
    secret: [u8; 16],
    started: Instant,
    cache: Vec<u8>,
    cached_at: Option<Instant>,
    budget_at: Instant,
    budget: u16,
}

impl Query {
    fn new() -> Self {
        Self {
            secret: *uuid::Uuid::new_v4().as_bytes(),
            started: Instant::now(),
            cache: Vec::new(),
            cached_at: None,
            budget_at: Instant::now(),
            budget: 128,
        }
    }

    fn token(&self, peer: SocketAddr, session: &[u8], epoch: u64) -> i32 {
        let mut hash = Sha256::new();
        hash.update(self.secret);
        hash.update(match peer.ip() {
            std::net::IpAddr::V4(ip) => ip.to_ipv6_mapped().octets(),
            std::net::IpAddr::V6(ip) => ip.octets(),
        });
        hash.update(peer.port().to_be_bytes());
        hash.update(session);
        hash.update(epoch.to_be_bytes());
        let digest = hash.finalize();
        i32::from_be_bytes([0, digest[0], digest[1], digest[2]])
    }

    fn request(&mut self, peer: SocketAddr, packet: &[u8]) -> Option<Request> {
        if self.budget_at.elapsed() >= Duration::from_secs(1) {
            self.budget = 128;
            self.budget_at = Instant::now();
        }
        if self.budget == 0 {
            return None;
        }
        self.budget -= 1;
        if packet.len() < 7 || packet[..2] != [0xfe, 0xfd] {
            return None;
        }
        let session: [u8; 4] = packet[3..7].try_into().ok()?;
        let epoch = self.started.elapsed().as_secs() / 30;
        match packet[2] {
            9 if packet.len() == 7 => {
                let mut response = vec![9];
                response.extend_from_slice(&session);
                string(
                    &mut response,
                    &self.token(peer, &session, epoch).to_string(),
                );
                Some(Request::Challenge(response))
            }
            0 if packet.len() == 11 || packet.len() == 15 => {
                let token = i32::from_be_bytes(packet[7..11].try_into().ok()?);
                if token != self.token(peer, &session, epoch)
                    && (epoch == 0 || token != self.token(peer, &session, epoch - 1))
                {
                    return None;
                }
                Some(Request::Stat {
                    session,
                    full: packet.len() == 15,
                })
            }
            _ => None,
        }
    }
}

enum Request {
    Challenge(Vec<u8>),
    Stat { session: [u8; 4], full: bool },
}

fn string(bytes: &mut Vec<u8>, value: &str) {
    bytes.extend(value.bytes().filter(|b| *b != 0));
    bytes.push(0);
}

fn stat(cfg: &crate::config::EdgeConfig, count: usize, players: &[String], full: bool) -> Vec<u8> {
    let mut bytes = vec![0; 5];
    let map = cfg
        .data
        .world_dir
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let count = count.to_string();
    let max = cfg.server.max_players.to_string();
    if full {
        bytes.extend_from_slice(b"splitnum\0\x80\0");
        for (key, value) in [
            ("hostname", cfg.server.motd.as_str()),
            ("gametype", "SMP"),
            ("game_id", "MINECRAFT"),
            ("version", mc_protocol::TARGET_RELEASE),
            ("plugins", "Solaris"),
            ("map", &map),
            ("numplayers", &count),
            ("maxplayers", &max),
            ("hostport", &cfg.network.port.to_string()),
            ("hostip", &cfg.network.bind_address),
        ] {
            string(&mut bytes, key);
            string(&mut bytes, value);
        }
        bytes.extend_from_slice(b"\0\x01player_\0\0");
        for player in players {
            string(&mut bytes, player);
        }
        bytes.push(0);
    } else {
        for value in [cfg.server.motd.as_str(), "SMP", &map, &count, &max] {
            string(&mut bytes, value);
        }
        bytes.extend_from_slice(&cfg.network.port.to_le_bytes());
        string(&mut bytes, &cfg.network.bind_address);
    }
    bytes
}

pub(super) async fn serve(
    socket: tokio::net::UdpSocket,
    server: Arc<EdgeServer>,
) -> anyhow::Result<()> {
    let mut query = Query::new();
    let mut packet = [0; 2048];
    loop {
        let (length, peer) = socket.recv_from(&mut packet).await?;
        let Some(request) = query.request(peer, &packet[..length]) else {
            continue;
        };
        let owned = match request {
            Request::Challenge(response) => Some(response),
            Request::Stat {
                session,
                full: true,
            } => {
                if query
                    .cached_at
                    .is_none_or(|at| at.elapsed() >= Duration::from_secs(5))
                {
                    let players = server.admin.roster(false);
                    query.cache = stat(&server.cfg, players.len(), &players, true);
                    query.cached_at = Some(Instant::now());
                }
                query.cache[1..5].copy_from_slice(&session);
                None
            }
            Request::Stat {
                session,
                full: false,
            } => {
                let mut response = stat(&server.cfg, server.connections.len(), &[], false);
                response[1..5].copy_from_slice(&session);
                Some(response)
            }
        };
        let response = owned.as_deref().unwrap_or(&query.cache);
        // GS4 has no multi-datagram response; never emit truncated player rows.
        if response.len() > 65507 {
            continue;
        }
        if let Err(error) = socket.send_to(response, peer).await {
            tracing::debug!(%error, "Query response failed");
        }
    }
}

#[cfg(test)]
#[path = "query_tests.rs"]
mod tests;
