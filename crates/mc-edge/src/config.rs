//! Edge v2 server configuration: TOML `--config server.toml` for `solaris-v2`.
//!
//! Reads the server configuration shared with the harness, restricted to the
//! keys the edge actually uses. Unknown keys are ignored with a one-line
//! `tracing::warn!` per file (never a startup error).

use std::path::{Path, PathBuf};

use serde::Deserialize;
use tracing::warn;

/// Full edge configuration, loaded from one TOML file.
#[derive(Debug, Clone, Deserialize)]
pub struct EdgeConfig {
    /// World identity: name, MOTD, player caps, view distances.
    #[serde(default)]
    pub server: ServerSection,
    /// Listener address and port.
    #[serde(default)]
    pub network: NetworkSection,
    /// World directory + seed for the interim dev backend.
    #[serde(default)]
    pub data: DataSection,
    /// Offline-mode flag (online-mode verification is a later milestone).
    #[serde(default)]
    pub auth: AuthSection,
    /// Optional read-only operator dashboard.
    #[serde(default)]
    pub dashboard: DashboardSection,
    /// Strict component package deployment; absent means no plugins.
    #[serde(default)]
    pub plugins: PluginsSection,
    /// Access lists live beside the explicit config, otherwise in the cwd.
    #[serde(skip, default = "default_server_dir")]
    pub(crate) server_dir: PathBuf,
}

impl Default for EdgeConfig {
    fn default() -> Self {
        toml::from_str("").expect("empty TOML parses into all-default sections")
    }
}
fn default_server_dir() -> PathBuf {
    PathBuf::from(".")
}

/// `[server]`: world identity and player/distance caps.
#[derive(Debug, Clone, Deserialize)]
pub struct ServerSection {
    /// Display name shown on the status ping and dashboard.
    #[serde(default = "default_name")]
    pub name: String,
    /// Message of the day shown on the status ping and dashboard.
    #[serde(default = "default_motd")]
    pub motd: String,
    /// Maximum concurrent Play sessions admitted.
    #[serde(default = "default_max_players")]
    pub max_players: u32,
    /// Upper bound on each client's negotiated chunk view radius.
    #[serde(default = "default_view_distance")]
    pub view_distance: u8,
    /// Gameplay simulation radius (mob ticking, random ticks, spawning).
    /// Residency follows view (+margin) ∪ active ∪ tickets ∪ dirty pins;
    /// simulation only decides which resident chunks are *active*.
    #[serde(default = "default_simulation_distance")]
    pub simulation_distance: u8,
}

impl Default for ServerSection {
    fn default() -> Self {
        Self {
            name: default_name(),
            motd: default_motd(),
            max_players: default_max_players(),
            view_distance: default_view_distance(),
            simulation_distance: default_simulation_distance(),
        }
    }
}

/// `[network]`: TCP listener.
#[derive(Debug, Clone, Deserialize)]
pub struct NetworkSection {
    /// IP to bind (default loopback; `0.0.0.0` accepts remote IPv4).
    #[serde(default = "default_bind_address")]
    pub bind_address: String,
    /// TCP port for Minecraft clients.
    #[serde(default = "default_port")]
    pub port: u16,
}

impl Default for NetworkSection {
    fn default() -> Self {
        Self {
            bind_address: default_bind_address(),
            port: default_port(),
        }
    }
}

/// `[data]`: world storage for the interim dev backend.
#[derive(Debug, Clone, Deserialize)]
pub struct DataSection {
    /// World directory (`region/`, `players/`, `journal/` live here).
    #[serde(default = "default_world_dir")]
    pub world_dir: PathBuf,
    /// World seed for chunk generation.
    #[serde(default)]
    pub seed: i64,
    /// Content-cache root; empty discovers XDG/home or checkout data/vanilla.
    #[serde(default = "default_vanilla_data_dir")]
    pub vanilla_data_dir: PathBuf,
}

impl Default for DataSection {
    fn default() -> Self {
        Self {
            world_dir: default_world_dir(),
            seed: 0,
            vanilla_data_dir: default_vanilla_data_dir(),
        }
    }
}

/// `[auth]`: authentication mode.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct AuthSection {
    /// `false` (default): offline mode, no Mojang verification.
    #[serde(default)]
    pub online_mode: bool,
    /// `false` (default): native sessions play immediately.
    /// `true`: new native sessions start locked and fail closed until the
    /// auth plugin unlocks them via `unlock_session`.
    #[serde(default)]
    pub require_plugin_login: bool,
}

/// `[plugins]`: Wasmtime component packages, resolved against the process cwd.
#[derive(Debug, Clone, Default, Deserialize)]
pub struct PluginsSection {
    pub directory: Option<PathBuf>,
}

/// `[dashboard]`: optional read-only operator dashboard.
#[derive(Debug, Clone, Deserialize)]
pub struct DashboardSection {
    /// Master switch (default off).
    #[serde(default)]
    pub enabled: bool,
    /// Bind address (default loopback).
    #[serde(default = "default_dashboard_bind")]
    pub bind_address: String,
    /// HTTP port.
    #[serde(default = "default_dashboard_port")]
    pub port: u16,
    /// Non-loopback binds additionally require this flag; without it the
    /// dashboard refuses to bind a remote address.
    #[serde(default)]
    pub allow_remote: bool,
}

impl Default for DashboardSection {
    fn default() -> Self {
        Self {
            enabled: false,
            bind_address: default_dashboard_bind(),
            port: default_dashboard_port(),
            allow_remote: false,
        }
    }
}

fn default_name() -> String {
    "solaris-edge".to_owned()
}
fn default_motd() -> String {
    "Solaris edge v2".to_owned()
}
fn default_max_players() -> u32 {
    20
}
fn default_view_distance() -> u8 {
    8
}
fn default_simulation_distance() -> u8 {
    6
}
fn default_bind_address() -> String {
    "127.0.0.1".to_owned()
}
fn default_port() -> u16 {
    25565
}
fn default_world_dir() -> PathBuf {
    PathBuf::from("world")
}
fn default_vanilla_data_dir() -> PathBuf {
    PathBuf::new()
}
fn default_dashboard_bind() -> String {
    "127.0.0.1".to_owned()
}
fn default_dashboard_port() -> u16 {
    8080
}

impl EdgeConfig {
    /// Load from a TOML file. Unknown keys are ignored with a warning naming
    /// the key path; missing sections take the defaults above. Relative
    /// `data.world_dir` / `data.vanilla_data_dir` resolve against the
    /// process working directory. The harness launches with `cwd` = repo root,
    /// so `data/vanilla` means the checkout's sidecar regardless of where the
    /// config file lives.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        let text = std::fs::read_to_string(path)?;
        Self::from_toml(&text, Some(path))
    }

    /// Parse TOML text; `origin` names the file in log lines (or `"config"`).
    /// Unknown keys are collected via a flatten catch-all and warned about.
    pub fn from_toml(text: &str, origin: Option<&Path>) -> anyhow::Result<Self> {
        let origin_name = origin.map_or_else(|| "config".to_owned(), |p| p.display().to_string());
        #[derive(Deserialize)]
        struct WithUnknown {
            #[serde(flatten)]
            known: EdgeConfig,
            #[serde(flatten)]
            unknown: toml::Table,
        }
        // `toml::Table` swallows every top-level key, so subtract the known
        // section names to find genuinely unknown ones.
        let parsed: WithUnknown = toml::from_str(text)?;
        for key in parsed.unknown.keys() {
            if !matches!(
                key.as_str(),
                "server" | "network" | "data" | "auth" | "dashboard" | "plugins"
            ) {
                warn!(origin = %origin_name, key = %key, "ignoring unknown config section");
            }
        }
        // Nested unknown keys: re-parse each known section strictly would
        // reject; instead walk the raw table and warn on unexpected leaves.
        if let Ok(table) = toml::from_str::<toml::Table>(text) {
            log_unknown_leaves(&origin_name, &table);
        }
        let mut cfg = parsed.known;
        cfg.server_dir = origin
            .and_then(Path::parent)
            .filter(|path| !path.as_os_str().is_empty())
            .map_or_else(default_server_dir, Path::to_path_buf);
        // Relative data paths resolve against the process working directory.
        // Absolute paths pass through untouched.
        let _ = origin;
        if cfg.server.view_distance == 0 {
            cfg.server.view_distance = default_view_distance();
        }
        if cfg.server.simulation_distance == 0 {
            cfg.server.simulation_distance = default_simulation_distance();
        }
        Ok(cfg)
    }

    /// Socket address the Minecraft listener binds.
    pub fn bind_addr(&self) -> String {
        format!("{}:{}", self.network.bind_address, self.network.port)
    }
}

/// Warn once per unknown leaf key under a known section (depth-first).
fn log_unknown_leaves(origin: &str, table: &toml::Table) {
    const KNOWN: &[(&str, &[&str])] = &[
        (
            "server",
            &[
                "name",
                "motd",
                "max_players",
                "view_distance",
                "simulation_distance",
            ],
        ),
        ("network", &["bind_address", "port"]),
        ("data", &["world_dir", "seed", "vanilla_data_dir"]),
        ("auth", &["online_mode", "require_plugin_login"]),
        ("plugins", &["directory"]),
        (
            "dashboard",
            &["enabled", "bind_address", "port", "allow_remote"],
        ),
    ];
    for (section, keys) in KNOWN {
        let Some(toml::Value::Table(inner)) = table.get(*section) else {
            continue;
        };
        for key in inner.keys() {
            if !keys.contains(&key.as_str()) {
                warn!(origin = %origin, key = %format!("{section}.{key}"), "ignoring unknown config key");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_config_takes_defaults() {
        let cfg = EdgeConfig::from_toml("", None).unwrap();
        assert_eq!(cfg.server.name, "solaris-edge");
        assert_eq!(cfg.network.port, 25565);
        assert!(!cfg.auth.online_mode);
        assert!(!cfg.auth.require_plugin_login);
        assert!(!cfg.dashboard.enabled);
    }

    #[test]
    fn require_plugin_login_defaults_off_and_parses_on() {
        let off = EdgeConfig::from_toml("[auth]\nonline_mode = false\n", None).unwrap();
        assert!(!off.auth.require_plugin_login);
        let on = EdgeConfig::from_toml("[auth]\nrequire_plugin_login = true\n", None).unwrap();
        assert!(on.auth.require_plugin_login);
    }

    #[test]
    fn known_keys_parse() {
        let cfg = EdgeConfig::from_toml(
            "[server]\nname = \"x\"\nmax_players = 5\nview_distance = 4\nsimulation_distance = 3\n\
             [network]\nport = 25566\n[data]\nseed = 81\n[auth]\nonline_mode = false\n\
             [dashboard]\nenabled = true\nport = 8090\n",
            None,
        )
        .unwrap();
        assert_eq!(cfg.server.name, "x");
        assert_eq!(cfg.server.max_players, 5);
        assert_eq!(cfg.server.view_distance, 4);
        assert_eq!(cfg.data.seed, 81);
        assert!(cfg.dashboard.enabled);
        assert_eq!(cfg.dashboard.port, 8090);
    }

    #[test]
    fn unknown_keys_ignored_not_rejected() {
        let cfg = EdgeConfig::from_toml(
            "[server]\nname = \"x\"\nfuture_flag = 1\n[plugins]\ndirectory = \"p\"\n",
            None,
        )
        .unwrap();
        assert_eq!(cfg.server.name, "x");
    }

    #[test]
    fn relative_paths_resolve_against_process_cwd_like_v1() {
        // v1 parity: relative data paths are NOT joined onto the config
        // file's directory; they resolve against the process cwd.
        let cfg = EdgeConfig::from_toml(
            "[data]\nworld_dir = \"w\"\n",
            Some(Path::new("/srv/mc/server.toml")),
        )
        .unwrap();
        assert_eq!(cfg.data.world_dir, PathBuf::from("w"));
        assert_eq!(cfg.server_dir, PathBuf::from("/srv/mc"));
        assert_eq!(EdgeConfig::default().server_dir, PathBuf::from("."));
    }
}
