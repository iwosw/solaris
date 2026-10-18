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

#[derive(Default)]
struct Lists {
    ops: Vec<OpEntry>,
    players: Vec<PlayerBan>,
    ips: Vec<IpBan>,
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

impl Admin {
    pub(crate) fn open(dir: &Path) -> anyhow::Result<Self> {
        Ok(Self {
            dir: dir.to_owned(),
            lists: Mutex::new(Lists {
                ops: load(&dir.join("ops.json"))?,
                players: load(&dir.join("banned-players.json"))?,
                ips: load(&dir.join("banned-ips.json"))?,
            }),
            sessions: Mutex::new(HashMap::new()),
            mutations: Mutex::new(()),
        })
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
    pub(crate) fn ban_player(
        &self,
        uuid: Uuid,
        name: &str,
        source: &str,
        reason: &str,
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
                expires: "forever".into(),
                reason: Some(reason.to_owned()),
            });
        }
        persist(&self.dir.join("banned-players.json"), &players)?;
        self.lists.lock().players = players;
        Ok(())
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
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
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
#[path = "admin_lock_tests.rs"]
mod lock_tests;
