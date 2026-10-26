//! Vanilla access lists and edge-owned administrative commands.
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use mc_core::boundary::ConnectionId;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct OpEntry {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default = "default_op_level")]
    pub level: u8,
    #[serde(default, rename = "bypassesPlayerLimit")]
    pub bypasses_player_limit: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct PlayerBan {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default = "now")]
    pub created: String,
    #[serde(default = "unknown_source")]
    pub source: String,
    #[serde(default = "forever")]
    pub expires: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct IpBan {
    pub ip: String,
    #[serde(default = "now")]
    pub created: String,
    #[serde(default = "unknown_source")]
    pub source: String,
    #[serde(default = "forever")]
    pub expires: String,
    #[serde(default)]
    pub reason: Option<String>,
}

/// One `whitelist.json` row: vanilla's own `UserWhiteListEntry` shape.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub(crate) struct WhitelistEntry {
    pub uuid: Uuid,
    pub name: String,
}

/// One `muted-players.json` row: Solaris-only chat mute with vanilla ban stamps.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub(crate) struct MuteEntry {
    pub uuid: Uuid,
    pub name: String,
    #[serde(default = "now")]
    pub created: String,
    #[serde(default = "unknown_source")]
    pub source: String,
    #[serde(default = "forever")]
    pub expires: String,
    #[serde(default)]
    pub reason: Option<String>,
}

#[derive(Default)]
struct Lists {
    ops: Vec<OpEntry>,
    players: Vec<PlayerBan>,
    ips: Vec<IpBan>,
    muted: Vec<MuteEntry>,
    whitelist: Vec<WhitelistEntry>,
    /// Whether login admission enforces `whitelist.json`.
    ///
    /// Vanilla keeps this in `server.properties`; Solaris keeps the persisted
    /// toggle in a `whitelist.enabled` file beside the list itself, so the
    /// list stays byte-compatible with vanilla.
    whitelist_enabled: bool,
}

pub(crate) struct Admin {
    dir: PathBuf,
    lists: Mutex<Lists>,
    sessions: Mutex<HashMap<ConnectionId, (Uuid, String, std::net::IpAddr, u8)>>,
    // Serialize read-modify-persist-swap without blocking admission or commands.
    mutations: Mutex<()>,
}

fn load<T: serde::de::DeserializeOwned>(path: &Path) -> anyhow::Result<Vec<T>> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

fn persist<T: Serialize>(path: &Path, entries: &[T]) -> anyhow::Result<()> {
    use std::io::Write;
    let temporary = path.with_extension("json.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(&serde_json::to_vec_pretty(entries)?)?;
    file.sync_all()?;
    std::fs::rename(temporary, path)?;
    std::fs::File::open(path.parent().unwrap_or(Path::new(".")))?.sync_all()?;
    Ok(())
}

/// The persisted `/whitelist on|off` toggle beside `whitelist.json`.
///
/// Vanilla stores the same flag in `server.properties`; the list file itself
/// stays a plain vanilla-compatible array so an existing `whitelist.json` can
/// be dropped in unchanged.
fn whitelist_enabled_path(dir: &Path) -> PathBuf {
    dir.join("whitelist.enabled")
}

fn read_whitelist_enabled(dir: &Path) -> bool {
    std::fs::read_to_string(whitelist_enabled_path(dir))
        .is_ok_and(|text| text.trim().eq_ignore_ascii_case("true"))
}

fn persist_whitelist_enabled(dir: &Path, enabled: bool) -> anyhow::Result<()> {
    use std::io::Write;
    let path = whitelist_enabled_path(dir);
    let temporary = path.with_extension("enabled.tmp");
    let mut file = std::fs::File::create(&temporary)?;
    file.write_all(if enabled { "true\n" } else { "false\n" }.as_bytes())?;
    file.sync_all()?;
    std::fs::rename(temporary, &path)?;
    std::fs::File::open(dir)?.sync_all()?;
    Ok(())
}

impl Admin {
    pub(crate) fn open(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            dir: dir.to_owned(),
            lists: Mutex::new(Lists {
                ops: load(&dir.join("ops.json"))?,
                players: load(&dir.join("banned-players.json"))?,
                ips: load(&dir.join("banned-ips.json"))?,
                muted: load(&dir.join("muted-players.json"))?,
                whitelist: load(&dir.join("whitelist.json"))?,
                whitelist_enabled: read_whitelist_enabled(dir),
            }),
            sessions: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
        })
    }

    pub(crate) fn operator_ids(&self) -> Vec<Uuid> {
        self.lists.lock().ops.iter().map(|entry| entry.uuid).collect()
    }

    pub(crate) fn level(&self, uuid: Uuid) -> u8 {
        self.lists
            .lock()
            .ops
            .iter()
            .find(|entry| entry.uuid == uuid)
            .map_or(0, |entry| entry.level.min(4))
    }

    pub(crate) fn admission(
        &self,
        uuid: Uuid,
        ip: Option<std::net::IpAddr>,
        full: bool,
    ) -> Option<serde_json::Value> {
        let lists = self.lists.lock();
        if let Some(entry) = lists
            .players
            .iter()
            .find(|entry| entry.uuid == uuid && active(&entry.expires))
        {
            return Some(ban_reason(false, entry.reason.as_deref(), &entry.expires));
        }
        if let Some(entry) = lists
            .ips
            .iter()
            .find(|entry| ip.is_some_and(|ip| entry.ip == ip.to_string()) && active(&entry.expires))
        {
            return Some(ban_reason(true, entry.reason.as_deref(), &entry.expires));
        }
        // Vanilla checks bans first and the whitelist second; an operator is
        // not exempt from the list. `multiplayer.disconnect.not_whitelisted`
        // is the client's own "You are not white-listed on this server!".
        if lists.whitelist_enabled && !lists.whitelist.iter().any(|entry| entry.uuid == uuid) {
            return Some(serde_json::json!({"translate":"multiplayer.disconnect.not_whitelisted"}));
        }
        if full
            && !lists
                .ops
                .iter()
                .any(|entry| entry.uuid == uuid && entry.bypasses_player_limit)
        {
            return Some(serde_json::json!({"translate":"multiplayer.disconnect.server_full"}));
        }
        None
    }

    pub(crate) fn register(
        &self,
        id: ConnectionId,
        uuid: Uuid,
        name: String,
        ip: std::net::IpAddr,
    ) {
        // Match set_op's lock order so a registration cannot cache an old level
        // after that operator mutation has committed.
        let lists = self.lists.lock();
        let level = lists
            .ops
            .iter()
            .find(|entry| entry.uuid == uuid)
            .map_or(0, |entry| entry.level.min(4));
        self.sessions.lock().insert(id, (uuid, name, ip, level));
    }
    pub(crate) fn remove(&self, id: ConnectionId) {
        self.sessions.lock().remove(&id);
    }
    pub(crate) fn uuid_of(&self, id: ConnectionId) -> Option<Uuid> {
        self.sessions.lock().get(&id).map(|session| session.0)
    }
    pub(crate) fn session_level(&self, id: ConnectionId) -> u8 {
        self.sessions.lock().get(&id).map_or(0, |session| session.3)
    }
    pub(crate) fn has_ip(&self, id: ConnectionId, ip: std::net::IpAddr) -> bool {
        self.sessions.lock().get(&id).is_some_and(|s| s.2 == ip)
    }
    pub(crate) fn roster(&self, uuids: bool) -> Vec<String> {
        let mut names: Vec<_> = self
            .sessions
            .lock()
            .values()
            .map(|s| {
                if uuids {
                    format!("{} ({})", s.1, s.0)
                } else {
                    s.1.clone()
                }
            })
            .collect();
        names.sort();
        names
    }
    pub(crate) fn profile(&self, name: &str) -> Option<(Uuid, String)> {
        let lists = self.lists.lock();
        lists
            .ops
            .iter()
            .find(|entry| entry.name.eq_ignore_ascii_case(name))
            .map(|entry| (entry.uuid, entry.name.clone()))
            .or_else(|| {
                lists
                    .players
                    .iter()
                    .find(|entry| entry.name.eq_ignore_ascii_case(name))
                    .map(|entry| (entry.uuid, entry.name.clone()))
            })
    }
    pub(crate) fn find(
        &self,
        name: &str,
    ) -> Option<(ConnectionId, Uuid, String, std::net::IpAddr)> {
        self.sessions
            .lock()
            .iter()
            .find(|(_, session)| session.1.eq_ignore_ascii_case(name))
            .map(|(id, s)| (*id, s.0, s.1.clone(), s.2))
    }

    /// Operator names in stable order for the management protocol.
    pub(crate) fn operator_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .lists
            .lock()
            .ops
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names
    }
    /// Persist and publish the `/whitelist on|off` toggle.
    pub(crate) fn set_whitelist(&self, enabled: bool) -> anyhow::Result<()> {
        let _mutation = self.mutations.lock();
        persist_whitelist_enabled(&self.dir, enabled)?;
        self.lists.lock().whitelist_enabled = enabled;
        Ok(())
    }

    /// Whitelisted names in stable order for `/whitelist list`.
    pub(crate) fn whitelist_names(&self) -> Vec<String> {
        let mut names: Vec<_> = self
            .lists
            .lock()
            .whitelist
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        names.sort();
        names
    }

    /// Add one identity to `whitelist.json`; false when it was already listed.
    pub(crate) fn whitelist_add(&self, uuid: Uuid, name: &str) -> anyhow::Result<bool> {
        let _mutation = self.mutations.lock();
        let mut whitelist = self.lists.lock().whitelist.clone();
        if whitelist.iter().any(|entry| entry.uuid == uuid) {
            return Ok(false);
        }
        whitelist.push(WhitelistEntry {
            uuid,
            name: name.to_owned(),
        });
        persist(&self.dir.join("whitelist.json"), &whitelist)?;
        self.lists.lock().whitelist = whitelist;
        Ok(true)
    }

    /// Remove every listing whose name matches; false when nothing matched.
    pub(crate) fn whitelist_remove(&self, name: &str) -> anyhow::Result<bool> {
        let _mutation = self.mutations.lock();
        let mut whitelist = self.lists.lock().whitelist.clone();
        if !whitelist
            .iter()
            .any(|entry| entry.name.eq_ignore_ascii_case(name))
        {
            return Ok(false);
        }
        whitelist.retain(|entry| !entry.name.eq_ignore_ascii_case(name));
        persist(&self.dir.join("whitelist.json"), &whitelist)?;
        self.lists.lock().whitelist = whitelist;
        Ok(true)
    }

    /// Re-read `whitelist.json` from disk, as vanilla `/whitelist reload` does.
    pub(crate) fn reload_whitelist(&self) -> anyhow::Result<usize> {
        let _mutation = self.mutations.lock();
        let whitelist: Vec<WhitelistEntry> = load(&self.dir.join("whitelist.json"))?;
        let count = whitelist.len();
        self.lists.lock().whitelist = whitelist;
        Ok(count)
    }

    /// Banned player and banned address identities, for `/banlist`.
    pub(crate) fn bans(&self) -> (Vec<String>, Vec<String>) {
        let lists = self.lists.lock();
        let players = lists
            .players
            .iter()
            .filter(|entry| active(&entry.expires))
            .map(|entry| format!("{} ({})", entry.name, entry.uuid))
            .collect();
        let ips = lists
            .ips
            .iter()
            .filter(|entry| active(&entry.expires))
            .map(|entry| entry.ip.clone())
            .collect();
        (players, ips)
    }

    pub(crate) fn set_op(&self, uuid: Uuid, name: &str, enabled: bool) -> anyhow::Result<()> {
        let _mutation = self.mutations.lock();
        let mut ops = {
            let lists = self.lists.lock();
            if enabled && lists.ops.iter().any(|entry| entry.uuid == uuid) {
                return Ok(());
            }
            lists.ops.clone()
        };
        ops.retain(|entry| entry.uuid != uuid);
        if enabled {
            ops.push(OpEntry {
                uuid,
                name: name.to_owned(),
                level: 4,
                bypasses_player_limit: false,
            });
        }
        persist(&self.dir.join("ops.json"), &ops)?;
        let mut lists = self.lists.lock();
        let mut sessions = self.sessions.lock();
        lists.ops = ops;
        for session in sessions.values_mut().filter(|session| session.0 == uuid) {
            session.3 = if enabled { 4 } else { 0 };
        }
        Ok(())
    }
    /// Permanently ban one identity (test convenience; commands call
    /// [`Self::ban_player_for`]).
    #[cfg(test)]
    pub(crate) fn ban_player(
        &self,
        uuid: Uuid,
        name: &str,
        source: &str,
        reason: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        self.ban_player_for(uuid, name, source, reason, None, enabled)
    }

    /// Ban one identity, optionally until `duration` has elapsed.
    pub(crate) fn ban_player_for(
        &self,
        uuid: Uuid,
        name: &str,
        source: &str,
        reason: &str,
        duration: Option<std::time::Duration>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let _mutation = self.mutations.lock();
        let mut players = self.lists.lock().players.clone();
        players.retain(|entry| entry.uuid != uuid);
        if enabled {
            players.push(PlayerBan {
                uuid,
                name: name.to_owned(),
                created: now(),
                source: source.to_owned(),
                expires: expires_after(duration),
                reason: Some(reason.to_owned()),
            });
        }
        persist(&self.dir.join("banned-players.json"), &players)?;
        self.lists.lock().players = players;
        Ok(())
    }

    /// Mute one identity for chat, optionally until `duration` has elapsed.
    pub(crate) fn mute_player(
        &self,
        uuid: Uuid,
        name: &str,
        source: &str,
        reason: &str,
        duration: Option<std::time::Duration>,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let _mutation = self.mutations.lock();
        let mut muted = self.lists.lock().muted.clone();
        muted.retain(|entry| entry.uuid != uuid);
        if enabled {
            muted.push(MuteEntry {
                uuid,
                name: name.to_owned(),
                created: now(),
                source: source.to_owned(),
                expires: expires_after(duration),
                reason: Some(reason.to_owned()),
            });
        }
        persist(&self.dir.join("muted-players.json"), &muted)?;
        self.lists.lock().muted = muted;
        Ok(())
    }

    /// Whether this identity may speak right now.
    pub(crate) fn muted(&self, uuid: Uuid) -> bool {
        self.lists
            .lock()
            .muted
            .iter()
            .any(|entry| entry.uuid == uuid && active(&entry.expires))
    }
    pub(crate) fn ban_ip(
        &self,
        ip: &str,
        source: &str,
        reason: &str,
        enabled: bool,
    ) -> anyhow::Result<()> {
        let _mutation = self.mutations.lock();
        let mut ips = self.lists.lock().ips.clone();
        ips.retain(|entry| entry.ip != ip);
        if enabled {
            ips.push(IpBan {
                ip: ip.into(),
                created: now(),
                source: source.into(),
                expires: "forever".into(),
                reason: Some(reason.into()),
            });
        }
        persist(&self.dir.join("banned-ips.json"), &ips)?;
        self.lists.lock().ips = ips;
        Ok(())
    }
}

// Vanilla's lists use UTC yyyy-MM-dd HH:mm:ss Z. Civil-date conversion
// avoids adding a time runtime dependency to the network edge.
fn now() -> String {
    format_utc(
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs(),
    )
}

/// The `expires` stamp of a temporary row: `forever` without a duration.
fn expires_after(duration: Option<std::time::Duration>) -> String {
    let Some(duration) = duration else {
        return forever();
    };
    let start = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format_utc(start.saturating_add(duration.as_secs()))
}

/// Parse a moderation duration such as `30m`, `1d`, `2h30m` or `1w`.
///
/// Vanilla's `/ban` has no duration; this is the usual `Xs/Xm/Xh/Xd/Xw`
/// vocabulary, additive in one token. Anything else is not a duration, so the
/// command treats it as the first word of the reason instead.
pub(crate) fn parse_duration(text: &str) -> Option<std::time::Duration> {
    const MAX_SECONDS: u64 = 3650 * 86_400;
    let mut seconds: u64 = 0;
    let mut current: Option<u64> = None;
    let mut segments = 0_u32;
    for byte in text.bytes() {
        match byte {
            b'0'..=b'9' => {
                let digit = u64::from(byte - b'0');
                current = Some(
                    current
                        .unwrap_or(0)
                        .saturating_mul(10)
                        .saturating_add(digit),
                );
            }
            unit => {
                let value = current.take()?;
                let scale = match unit {
                    b's' => 1,
                    b'm' => 60,
                    b'h' => 3_600,
                    b'd' => 86_400,
                    b'w' => 604_800,
                    _ => return None,
                };
                if value == 0 {
                    return None;
                }
                seconds = seconds.checked_add(value.checked_mul(scale)?)?;
                segments += 1;
            }
        }
    }
    if current.is_some() || segments == 0 || seconds == 0 || seconds > MAX_SECONDS {
        return None;
    }
    Some(std::time::Duration::from_secs(seconds))
}

/// Vanilla's UTC `yyyy-MM-dd HH:mm:ss +0000` stamp for one Unix second.
fn format_utc(seconds: u64) -> String {
    let z = (seconds / 86400) as i64 + 719468;
    let era = z / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let mut year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = mp + if mp < 10 { 3 } else { -9 };
    year += i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} +0000",
        seconds / 3600 % 24,
        seconds / 60 % 60,
        seconds % 60
    )
}
fn default_op_level() -> u8 {
    4
}
fn unknown_source() -> String {
    "(Unknown)".into()
}
fn forever() -> String {
    "forever".into()
}

fn expiry_seconds(expires: &str) -> Option<i64> {
    if expires.len() != 25 || !expires.is_ascii() {
        return None;
    }
    let mut year = expires.get(0..4)?.parse::<i64>().ok()?;
    let month = expires.get(5..7)?.parse::<i64>().ok()?;
    let day = expires.get(8..10)?.parse::<i64>().ok()?;
    let hour = expires.get(11..13)?.parse::<i64>().ok()?;
    let minute = expires.get(14..16)?.parse::<i64>().ok()?;
    let second = expires.get(17..19)?.parse::<i64>().ok()?;
    let zone = expires.get(20..25)?;
    if zone.len() != 5 || !matches!(zone.as_bytes()[0], b'+' | b'-') {
        return None;
    }
    let zone_hour = zone.get(1..3)?.parse::<i64>().ok()?;
    let zone_minute = zone.get(3..5)?.parse::<i64>().ok()?;
    if !(1..=12).contains(&month)
        || !(1..=31).contains(&day)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return None;
    }
    year -= i64::from(month <= 2);
    let era = year.div_euclid(400);
    let yoe = year - era * 400;
    let mp = month + if month > 2 { -3 } else { 9 };
    let doy = (153 * mp + 2) / 5 + day - 1;
    let days = era * 146097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719468;
    let offset = (zone_hour * 60 + zone_minute) * 60 * if zone.starts_with('-') { -1 } else { 1 };
    Some(days * 86400 + hour * 3600 + minute * 60 + second - offset)
}

fn active(expires: &str) -> bool {
    let current = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    expiry_seconds(expires).is_none_or(|expiry| expiry >= current)
}

fn ban_reason(ip: bool, reason: Option<&str>, expires: &str) -> serde_json::Value {
    let prefix = if ip {
        "multiplayer.disconnect.banned_ip"
    } else {
        "multiplayer.disconnect.banned"
    };
    let reason = reason.map(|text| serde_json::json!(text)).unwrap_or_else(
        || serde_json::json!({"translate":"multiplayer.disconnect.banned.reason.default"}),
    );
    let mut component =
        serde_json::json!({"translate":format!("{prefix}.reason"), "with":[reason]});
    if expiry_seconds(expires).is_some() {
        let display = format!(
            "{} at {} GMT{}:{}",
            &expires[..10],
            &expires[11..19],
            &expires[20..23],
            &expires[23..25]
        );
        component["extra"] =
            serde_json::json!([{"translate":format!("{prefix}.expiration"), "with":[display]}]);
    }
    component
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ops_round_trip_and_banned_uuid_admission() {
        let dir = std::env::temp_dir().join(format!("solaris-admin-{}", Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let uuid = Uuid::new_v4();
        let admin = Admin::open(&dir).unwrap();
        admin.set_op(uuid, "Operator", true).unwrap();
        let reopened = Admin::open(&dir).unwrap();
        assert_eq!(reopened.level(uuid), 4);
        let wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.join("ops.json")).unwrap()).unwrap();
        assert_eq!(wire[0]["bypassesPlayerLimit"], false);
        reopened
            .ban_player(uuid, "Operator", "Console", "Banned by an operator.", true)
            .unwrap();
        assert_eq!(
            Admin::open(&dir)
                .unwrap()
                .admission(uuid, None, false)
                .unwrap()["translate"],
            "multiplayer.disconnect.banned.reason"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
}

#[cfg(test)]
mod expiry_tests {
    use super::*;
    #[test]
    fn vanilla_dates_honor_timezone_and_invalid_forever() {
        assert_eq!(expiry_seconds("1970-01-01 00:00:00 +0000"), Some(0));
        assert_eq!(expiry_seconds("1970-01-01 00:00:00 -0100"), Some(3600));
        assert_eq!(expiry_seconds("1970-01-01 01:00:00 +0100"), Some(0));
        assert!(!active("2000-01-01 00:00:00 +0000"));
        assert!(active("forever"));
        assert!(active("unparseable"));
    }

    #[test]
    fn vanilla_missing_defaults_and_null_reason() {
        let uuid = Uuid::nil();
        let op: OpEntry =
            serde_json::from_value(serde_json::json!({"uuid":uuid,"name":"Operator"})).unwrap();
        assert_eq!(op.level, 4);
        assert!(!op.bypasses_player_limit);
        let ban: PlayerBan =
            serde_json::from_value(serde_json::json!({"uuid":uuid,"name":"Banned","reason":null}))
                .unwrap();
        assert_eq!(ban.expires, "forever");
        assert_eq!(
            ban_reason(false, ban.reason.as_deref(), &ban.expires)["with"][0]["translate"],
            "multiplayer.disconnect.banned.reason.default"
        );
    }
}

#[cfg(test)]
mod whitelist_tests {
    use super::*;
    use std::net::IpAddr;

    fn fixture() -> (Admin, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Admin::open(dir.path()).unwrap(), dir)
    }

    #[test]
    fn enabled_whitelist_admits_only_listed_identities() {
        let (admin, dir) = fixture();
        let listed = Uuid::new_v4();
        let stranger = Uuid::new_v4();
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(admin.admission(stranger, Some(ip), false).is_none());
        admin.set_whitelist(true).unwrap();
        assert_eq!(
            admin.admission(stranger, Some(ip), false).unwrap()["translate"],
            "multiplayer.disconnect.not_whitelisted"
        );
        assert!(admin.whitelist_add(listed, "Listed").unwrap());
        assert!(admin.admission(listed, Some(ip), false).is_none());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn whitelist_file_is_a_vanilla_array_and_toggle_survives_reopen() {
        let (admin, dir) = fixture();
        let uuid = Uuid::new_v4();
        admin.set_whitelist(true).unwrap();
        assert!(admin.whitelist_add(uuid, "Listed").unwrap());
        assert!(!admin.whitelist_add(uuid, "Listed").unwrap());
        let wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("whitelist.json")).unwrap())
                .unwrap();
        assert_eq!(wire[0]["uuid"], uuid.to_string());
        assert_eq!(wire[0]["name"], "Listed");
        assert_eq!(wire.as_array().unwrap()[0].as_object().unwrap().len(), 2);
        let reopened = Admin::open(dir.path()).unwrap();
        assert!(reopened.admission(uuid, None, false).is_none());
        assert_eq!(
            reopened.admission(Uuid::new_v4(), None, false).unwrap()["translate"],
            "multiplayer.disconnect.not_whitelisted"
        );
        assert_eq!(reopened.whitelist_names(), vec!["Listed".to_owned()]);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn reload_reads_hand_edited_vanilla_list_and_remove_matches_name_case() {
        let (admin, dir) = fixture();
        let uuid = Uuid::new_v4();
        admin.whitelist_add(uuid, "Listed").unwrap();
        std::fs::write(
            dir.path().join("whitelist.json"),
            serde_json::to_vec(&vec![serde_json::json!({
                "uuid": Uuid::new_v4(),
                "name": "Edited"
            })])
            .unwrap(),
        )
        .unwrap();
        assert_eq!(admin.reload_whitelist().unwrap(), 1);
        assert_eq!(admin.whitelist_names(), vec!["Edited".to_owned()]);
        assert!(admin.whitelist_remove("eDiTeD").unwrap());
        assert!(!admin.whitelist_remove("Edited").unwrap());
        assert!(admin.whitelist_names().is_empty());
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn banlist_skips_expired_temporary_bans() {
        let (admin, dir) = fixture();
        let active = Uuid::new_v4();
        let expired = Uuid::new_v4();
        admin
            .ban_player(active, "Active", "Console", "test", true)
            .unwrap();
        std::fs::write(
            dir.path().join("banned-players.json"),
            serde_json::to_vec(&vec![
                serde_json::json!({"uuid": active, "name": "Active"}),
                serde_json::json!({
                    "uuid": expired,
                    "name": "Expired",
                    "expires": "2000-01-01 00:00:00 +0000"
                }),
            ])
            .unwrap(),
        )
        .unwrap();
        admin.ban_ip("10.0.0.1", "Console", "test", true).unwrap();
        let reopened = Admin::open(dir.path()).unwrap();
        let (players, ips) = reopened.bans();
        assert_eq!(players, vec![format!("Active ({active})")]);
        assert_eq!(ips, vec!["10.0.0.1".to_owned()]);
        std::fs::remove_dir_all(dir.path()).unwrap();
    }
}

#[cfg(test)]
mod moderation_tests {
    use super::*;

    fn fixture() -> (Admin, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (Admin::open(dir.path()).unwrap(), dir)
    }

    #[test]
    fn durations_parse_as_the_usual_d_moderation_vocabulary() {
        assert_eq!(
            parse_duration("30m"),
            Some(std::time::Duration::from_secs(1_800))
        );
        assert_eq!(
            parse_duration("1d"),
            Some(std::time::Duration::from_secs(86_400))
        );
        assert_eq!(
            parse_duration("2h30m"),
            Some(std::time::Duration::from_secs(9_000))
        );
        assert_eq!(
            parse_duration("1w2d3h4m5s"),
            Some(std::time::Duration::from_secs(
                604_800 + 172_800 + 10_800 + 240 + 5
            ))
        );
        for rejected in ["", "griefing", "1", "0s", "30m10", "d", "-1d", "1y", "1 d"] {
            assert_eq!(parse_duration(rejected), None, "{rejected:?}");
        }
    }

    #[test]
    fn temporary_ban_uses_vanilla_expires_and_expires_the_admission_refusal() {
        let (admin, dir) = fixture();
        let uuid = Uuid::new_v4();
        admin
            .ban_player_for(
                uuid,
                "Alex",
                "Console",
                "test",
                Some(std::time::Duration::from_secs(3_600)),
                true,
            )
            .unwrap();
        let wire: serde_json::Value =
            serde_json::from_slice(&std::fs::read(dir.path().join("banned-players.json")).unwrap())
                .unwrap();
        let expires = wire[0]["expires"].as_str().unwrap().to_owned();
        assert_ne!(expires, "forever");
        assert!(expiry_seconds(&expires).is_some());
        let refusal = admin.admission(uuid, None, false).unwrap();
        assert_eq!(refusal["translate"], "multiplayer.disconnect.banned.reason");
        assert_eq!(
            refusal["extra"][0]["translate"],
            "multiplayer.disconnect.banned.expiration"
        );
        std::fs::remove_dir_all(dir.path()).unwrap();
    }

    #[test]
    fn mute_rows_persist_and_active_only() {
        let (admin, dir) = fixture();
        let muted = Uuid::new_v4();
        let expired = Uuid::new_v4();
        admin
            .mute_player(
                muted,
                "Muted",
                "Console",
                "spam",
                Some(std::time::Duration::from_secs(3_600)),
                true,
            )
            .unwrap();
        assert!(admin.muted(muted));
        let path = dir.path().join("muted-players.json");
        let mut rows: Vec<serde_json::Value> =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        rows.push(serde_json::json!({
            "uuid": expired,
            "name": "Expired",
            "expires": "2000-01-01 00:00:00 +0000"
        }));
        std::fs::write(&path, serde_json::to_vec(&rows).unwrap()).unwrap();
        let reopened = Admin::open(dir.path()).unwrap();
        assert!(reopened.muted(muted));
        assert!(!reopened.muted(expired));
        admin
            .mute_player(muted, "Muted", "Console", "spam", None, false)
            .unwrap();
        assert!(!Admin::open(dir.path()).unwrap().muted(muted));
        std::fs::remove_dir_all(dir.path()).unwrap();
    }
}

#[cfg(test)]
#[path = "admin_lock_tests.rs"]
mod lock_tests;

#[cfg(test)]
#[path = "access_compat_tests.rs"]
mod access_compat_tests;
