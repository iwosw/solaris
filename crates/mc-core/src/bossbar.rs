//! Vanilla custom boss bars (`/bossbar`), owned by the tick thread.
//!
//! Server-global registry (bar id -> bar), moved between dimension worlds by
//! `dimensions_tick::swap_globals`. Vanilla persists this in level.dat;
//! Solaris keeps it session-local (see open issues).

use std::collections::{BTreeMap, BTreeSet};

use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::boundary::{BossBarChange, ConnectionId, EntityId, PlayerChange};
use crate::tick::TickState;

/// Vanilla `BossEvent.BossBarColor` ordinals (declaration order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum BarColor {
    Pink = 0,
    Blue = 1,
    Red = 2,
    Green = 3,
    Yellow = 4,
    Purple = 5,
    #[default]
    White = 6,
}

impl BarColor {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "pink" => Self::Pink,
            "blue" => Self::Blue,
            "red" => Self::Red,
            "green" => Self::Green,
            "yellow" => Self::Yellow,
            "purple" => Self::Purple,
            "white" => Self::White,
            _ => return None,
        })
    }
}

/// Vanilla `BossEvent.BossBarOverlay` ordinals (declaration order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(i32)]
pub enum BarOverlay {
    #[default]
    Progress = 0,
    Notched6 = 1,
    Notched10 = 2,
    Notched12 = 3,
    Notched20 = 4,
}

impl BarOverlay {
    pub fn parse(word: &str) -> Option<Self> {
        Some(match word {
            "progress" => Self::Progress,
            "notched_6" => Self::Notched6,
            "notched_10" => Self::Notched10,
            "notched_12" => Self::Notched12,
            "notched_20" => Self::Notched20,
            _ => return None,
        })
    }
}

/// One custom boss bar. `players` holds member player UUIDs (vanilla
/// `CustomBossEvent.players`); packet fan-out resolves them to connections.
#[derive(Debug, Clone, PartialEq)]
pub struct BossBar {
    pub id: String,
    pub name: mc_nbt::Tag,
    pub value: i32,
    pub max: i32,
    pub color: BarColor,
    pub overlay: BarOverlay,
    pub visible: bool,
    pub players: BTreeSet<Uuid>,
}

impl BossBar {
    pub fn progress(&self) -> f32 {
        if self.max <= 0 {
            return 0.0;
        }
        (f64::from(self.value) / f64::from(self.max)).clamp(0.0, 1.0) as f32
    }

    pub fn uuid(&self) -> Uuid {
        bar_uuid(&self.id)
    }
}

/// Stable per-bar UUID derived deterministically from the bar id string, so
/// repeated add/remove cycles address the same client-side bar (SHA-256
/// truncated, version/variant bits set; no uuid-crate feature needed).
pub fn bar_uuid(id: &str) -> Uuid {
    let digest = Sha256::digest(id.as_bytes());
    let mut bytes: [u8; 16] = digest[..16].try_into().expect("sha256 is 32 bytes");
    bytes[6] = (bytes[6] & 0x0f) | 0x50;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[derive(Debug, Default, Clone, PartialEq)]
pub struct BossBars {
    pub bars: BTreeMap<String, BossBar>,
}

impl BossBars {
    pub fn get(&self, id: &str) -> Option<&BossBar> {
        self.bars.get(id)
    }

    /// Named-NBT bytes of [`Self::to_nbt`] for journal images (Tag has no serde).
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::new();
        mc_nbt::write_named(&mut bytes, "", &self.to_nbt())
            .expect("in-memory NBT encode cannot fail");
        bytes
    }

    /// Decode [`Self::to_bytes`] output, fail-closed like checkpoints.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let mut input = bytes;
        let (_, tag) = mc_nbt::read_named(&mut input)
            .map_err(|error| format!("invalid bossbar image: {error}"))?;
        if !input.is_empty() {
            return Err("invalid bossbar image: trailing bytes".into());
        }
        Self::from_nbt(&tag)
    }
}

/// Vanilla `CustomBossEvents` saved-data layout (`minecraft:custom_boss_events`):
/// the id maps to `{Name, Value, Max, Color, Overlay, Visible, Players, ...}`.
/// `Name` is stored as the resolved network component tag. Unknown or corrupt
/// records fail closed (like `scoreboard_persistence`), never silently dropped.
const COLORS: [&str; 7] = ["pink", "blue", "red", "green", "yellow", "purple", "white"];
const OVERLAYS: [&str; 5] = [
    "progress",
    "notched_6",
    "notched_10",
    "notched_12",
    "notched_20",
];

type Fields = [(String, mc_nbt::Tag)];

fn compound(tag: &mc_nbt::Tag) -> Result<&Fields, String> {
    match tag {
        mc_nbt::Tag::Compound(fields) => Ok(fields),
        _ => Err("expected bossbar compound".into()),
    }
}

fn field<'a>(fields: &'a Fields, key: &str) -> Result<Option<&'a mc_nbt::Tag>, String> {
    let mut it = fields.iter().filter(|(name, _)| name == key);
    let value = it.next().map(|(_, tag)| tag);
    if it.next().is_some() {
        return Err(format!("duplicate bossbar field {key}"));
    }
    Ok(value)
}

fn required<'a>(fields: &'a Fields, key: &str) -> Result<&'a mc_nbt::Tag, String> {
    field(fields, key)?.ok_or_else(|| format!("missing bossbar field {key}"))
}

fn boolean(fields: &Fields, key: &str, default: bool) -> Result<bool, String> {
    match field(fields, key)? {
        None => Ok(default),
        Some(mc_nbt::Tag::Byte(0)) => Ok(false),
        Some(mc_nbt::Tag::Byte(1)) => Ok(true),
        _ => Err(format!("invalid bossbar boolean {key}")),
    }
}

fn list<'a>(fields: &'a Fields, key: &str, kind: u8) -> Result<&'a [mc_nbt::Tag], String> {
    match field(fields, key)? {
        None => Ok(&[]),
        Some(mc_nbt::Tag::List(list))
            if list.element_type == kind
                || (list.elements.is_empty() && list.element_type == mc_nbt::tag_type::END) =>
        {
            Ok(&list.elements)
        }
        _ => Err(format!("invalid bossbar list {key}")),
    }
}

fn uuid_of(tag: &mc_nbt::Tag) -> Result<Uuid, String> {
    match tag {
        mc_nbt::Tag::IntArray(words) if words.len() == 4 => {
            let value = words.iter().fold(0u128, |accumulated, word| {
                (accumulated << 32) | u128::from(*word as u32)
            });
            Ok(Uuid::from_u128(value))
        }
        _ => Err("invalid bossbar player UUID".into()),
    }
}

fn uuid_tag(id: Uuid) -> mc_nbt::Tag {
    let value = id.as_u128();
    mc_nbt::Tag::IntArray(
        (0..4)
            .map(|index| (value >> (96 - 32 * index)) as i32)
            .collect(),
    )
}

fn color_of(tag: &mc_nbt::Tag) -> Result<BarColor, String> {
    match tag {
        mc_nbt::Tag::String(name) => COLORS
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| match index {
                0 => BarColor::Pink,
                1 => BarColor::Blue,
                2 => BarColor::Red,
                3 => BarColor::Green,
                4 => BarColor::Yellow,
                5 => BarColor::Purple,
                _ => BarColor::White,
            })
            .ok_or_else(|| format!("unknown bossbar color {name}")),
        _ => Err("invalid bossbar color".into()),
    }
}

fn overlay_of(tag: &mc_nbt::Tag) -> Result<BarOverlay, String> {
    match tag {
        mc_nbt::Tag::String(name) => OVERLAYS
            .iter()
            .position(|candidate| *candidate == name)
            .map(|index| match index {
                0 => BarOverlay::Progress,
                1 => BarOverlay::Notched6,
                2 => BarOverlay::Notched10,
                3 => BarOverlay::Notched12,
                _ => BarOverlay::Notched20,
            })
            .ok_or_else(|| format!("unknown bossbar style {name}")),
        _ => Err("invalid bossbar style".into()),
    }
}

impl BossBars {
    /// Encode the full vanilla saved-data envelope, including DataVersion.
    pub fn to_nbt(&self) -> mc_nbt::Tag {
        let bars: Vec<mc_nbt::Tag> = self
            .bars
            .iter()
            .map(|(id, bar)| {
                mc_nbt::Tag::Compound(vec![
                    ("Id".into(), mc_nbt::Tag::String(id.clone())),
                    ("Name".into(), bar.name.clone()),
                    ("Value".into(), mc_nbt::Tag::Int(bar.value)),
                    ("Max".into(), mc_nbt::Tag::Int(bar.max)),
                    (
                        "Color".into(),
                        mc_nbt::Tag::String(COLORS[bar.color as usize].to_owned()),
                    ),
                    (
                        "Overlay".into(),
                        mc_nbt::Tag::String(OVERLAYS[bar.overlay as usize].to_owned()),
                    ),
                    ("Visible".into(), mc_nbt::Tag::Byte(i8::from(bar.visible))),
                    ("DarkenScreen".into(), mc_nbt::Tag::Byte(0)),
                    ("PlayBossMusic".into(), mc_nbt::Tag::Byte(0)),
                    ("CreateWorldFog".into(), mc_nbt::Tag::Byte(0)),
                    (
                        "Players".into(),
                        mc_nbt::Tag::List(mc_nbt::ListTag {
                            element_type: mc_nbt::tag_type::INT_ARRAY,
                            elements: bar.players.iter().map(|player| uuid_tag(*player)).collect(),
                        }),
                    ),
                ])
            })
            .collect();
        mc_nbt::Tag::Compound(vec![
            (
                "DataVersion".into(),
                mc_nbt::Tag::Int(mc_world::anvil::TARGET_DATA_VERSION),
            ),
            (
                "data".into(),
                mc_nbt::Tag::Compound(vec![(
                    "Bars".into(),
                    mc_nbt::Tag::List(mc_nbt::ListTag {
                        element_type: mc_nbt::tag_type::COMPOUND,
                        elements: bars,
                    }),
                )]),
            ),
        ])
    }

    /// Decode vanilla saved data without treating malformed records as absent.
    pub fn from_nbt(root: &mc_nbt::Tag) -> Result<Self, String> {
        let root = compound(root)?;
        if let Some(version) = field(root, "DataVersion")?
            && !matches!(version, mc_nbt::Tag::Int(_))
        {
            return Err("invalid bossbar DataVersion".into());
        }
        let data = compound(required(root, "data")?)?;
        let mut result = Self::default();
        for (index, tag) in list(data, "Bars", mc_nbt::tag_type::COMPOUND)?
            .iter()
            .enumerate()
        {
            let decode = || -> Result<(String, BossBar), String> {
                let fields = compound(tag)?;
                let id = match required(fields, "Id")? {
                    mc_nbt::Tag::String(id) => {
                        mc_data::Identifier::parse(id)
                            .map(|parsed| parsed.as_str().to_owned())
                            .map_err(|_| format!("invalid bossbar id {id}"))?;
                        id.clone()
                    }
                    _ => return Err("invalid bossbar id".into()),
                };
                let name = required(fields, "Name")?.clone();
                if !matches!(
                    name,
                    mc_nbt::Tag::String(_) | mc_nbt::Tag::Compound(_) | mc_nbt::Tag::List(_)
                ) {
                    return Err("invalid bossbar name".into());
                }
                let value = match required(fields, "Value")? {
                    mc_nbt::Tag::Int(value) if *value >= 0 => *value,
                    _ => return Err("invalid bossbar value".into()),
                };
                let max = match required(fields, "Max")? {
                    mc_nbt::Tag::Int(max) if *max >= 1 => *max,
                    _ => return Err("invalid bossbar max".into()),
                };
                if boolean(fields, "DarkenScreen", false)?
                    || boolean(fields, "PlayBossMusic", false)?
                    || boolean(fields, "CreateWorldFog", false)?
                {
                    return Err("unsupported bossbar flags".into());
                }
                let mut players = BTreeSet::new();
                for tag in list(fields, "Players", mc_nbt::tag_type::INT_ARRAY)? {
                    if !players.insert(uuid_of(tag)?) {
                        return Err("duplicate bossbar player".into());
                    }
                }
                Ok((
                    id.clone(),
                    BossBar {
                        id,
                        name,
                        value,
                        max,
                        color: color_of(required(fields, "Color")?)?,
                        overlay: overlay_of(required(fields, "Overlay")?)?,
                        visible: boolean(fields, "Visible", true)?,
                        players,
                    },
                ))
            };
            let (id, bar) = decode().map_err(|error| format!("bar-index {index}: {error}"))?;
            if result.bars.insert(id.clone(), bar).is_some() {
                return Err(format!("bar-index {index}: duplicate bossbar id {id}"));
            }
        }
        Ok(result)
    }
}

fn text(value: impl ToString) -> mc_nbt::Tag {
    mc_nbt::Tag::Compound(vec![(
        "text".into(),
        mc_nbt::Tag::String(value.to_string()),
    )])
}

pub(crate) fn feedback(
    state: &mut TickState,
    connection: ConnectionId,
    key: &str,
    arguments: Vec<mc_nbt::Tag>,
) {
    crate::command_execution::feedback(state, connection, key, arguments);
}

/// `CommandSourceStack.sendFailure`: red translated component.
fn failure(state: &mut TickState, connection: ConnectionId, key: &str) {
    let content = mc_nbt::Tag::Compound(vec![
        ("translate".into(), mc_nbt::Tag::String(key.into())),
        ("color".into(), mc_nbt::Tag::String("red".into())),
    ]);
    state.push_player(connection, PlayerChange::StyledChat { content });
}

fn error_text(state: &mut TickState, connection: ConnectionId, message: String) {
    state.push_player(connection, PlayerChange::Chat { message });
}

fn format_list(names: Vec<mc_nbt::Tag>) -> mc_nbt::Tag {
    mc_nbt::Tag::List(mc_nbt::ListTag {
        element_type: mc_nbt::tag_type::COMPOUND,
        elements: names,
    })
}

fn add_packet(bar: &BossBar) -> BossBarChange {
    BossBarChange::Add {
        name: bar.name.clone(),
        progress: bar.progress(),
        color: bar.color as i32,
        overlay: bar.overlay as i32,
        flags: 0,
    }
}

/// Push `change` for `bar_id` to member connections in this world and relay
/// it to member connections in other worlds (scoreboard `publish` pattern:
/// direct local fan-out plus a `Broadcast` the coordinator relays).
fn send_to_members(
    state: &mut TickState,
    players: &BTreeSet<Uuid>,
    bar_id: Uuid,
    change: &BossBarChange,
) {
    let local: Vec<ConnectionId> = players
        .iter()
        .filter_map(|uuid| {
            state
                .connections
                .all_connections()
                .find(|connection| state.connections.uuid_of(*connection) == Some(*uuid))
        })
        .collect();
    for connection in local {
        state.push_player(
            connection,
            PlayerChange::BossBar {
                id: bar_id,
                change: change.clone(),
            },
        );
    }
    if state.dimension_globals.active {
        state
            .dimension_globals
            .broadcasts
            .push(crate::dimensions::globals::Broadcast::BossBar {
                players: players.iter().copied().collect(),
                id: bar_id,
                change: change.clone(),
            });
    }
}

/// Resolve a `players` target word to member UUIDs. Supports plain player
/// names and `@a` across all dimensions (via the global roster); other
/// selectors resolve against the executing world through the shared selector
/// engine, cross-checked to players. Distant-dimension positional selectors
/// (`@p`, `@e[...]`, ...) only see the executing world.
pub(crate) fn resolve_member_uuids(
    state: &TickState,
    executor: EntityId,
    word: &str,
) -> Result<Vec<Uuid>, &'static str> {
    if word == "@a" {
        let mut uuids: Vec<Uuid> = state
            .dimension_globals
            .roster
            .iter()
            .map(|player| player.uuid)
            .collect();
        for world in
            std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            for connection in world.connections.all_connections() {
                if let Some(uuid) = world.connections.uuid_of(connection)
                    && !uuids.contains(&uuid)
                {
                    uuids.push(uuid);
                }
            }
        }
        return Ok(uuids);
    }
    if !word.starts_with('@') {
        for world in
            std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            for connection in world.connections.all_connections() {
                let entity = world
                    .connections
                    .entity_of(connection)
                    .expect("joined entity");
                if world.connections.name_of(entity) == Some(word) {
                    return Ok(vec![
                        world.connections.uuid_of(connection).expect("joined UUID"),
                    ]);
                }
            }
        }
        if let Some(player) = state
            .dimension_globals
            .roster
            .iter()
            .find(|player| player.name == word)
        {
            return Ok(vec![player.uuid]);
        }
        return Err("argument.entity.notfound.player");
    }
    let target = crate::commands::target_of(word).map_err(|_| "argument.entity.notfound.player")?;
    let mut uuids = Vec::new();
    for id in crate::command_execution::resolve_target(state, executor, &target) {
        let mut found = None;
        for world in
            std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
        {
            if let Some(connection) = world.connections.entity_connection(id) {
                found = world.connections.uuid_of(connection);
                break;
            }
        }
        if let Some(uuid) = found
            && !uuids.contains(&uuid)
        {
            uuids.push(uuid);
        }
    }
    if uuids.is_empty() {
        return Err("argument.entity.notfound.player");
    }
    Ok(uuids)
}

/// Display names of member UUIDs for `some`-style feedback.
pub(crate) fn member_names(state: &TickState, uuids: &[Uuid]) -> Vec<mc_nbt::Tag> {
    uuids
        .iter()
        .map(|uuid| {
            for world in
                std::iter::once(state).chain(state.other_dimensions.values().map(|world| &**world))
            {
                for connection in world.connections.all_connections() {
                    if world.connections.uuid_of(connection) == Some(*uuid) {
                        let entity = world
                            .connections
                            .entity_of(connection)
                            .expect("joined entity");
                        if let Some(name) = world.connections.name_of(entity) {
                            return text(name);
                        }
                    }
                }
            }
            if let Some(player) = state
                .dimension_globals
                .roster
                .iter()
                .find(|player| &player.uuid == uuid)
            {
                return text(&player.name);
            }
            text(uuid.to_string())
        })
        .collect()
}

fn unknown(state: &mut TickState, connection: ConnectionId, id: &str) {
    feedback(
        state,
        connection,
        "commands.bossbar.unknown",
        vec![text(id)],
    );
}
pub(crate) fn execute(
    state: &mut TickState,
    connection: ConnectionId,
    executor: EntityId,
    command: &crate::command_bossbar::BossBarCommand,
) {
    use crate::command_bossbar::BossBarCommand as Cmd;
    match command {
        Cmd::Add { id, name } => {
            if state.bossbars.bars.contains_key(id) {
                feedback(
                    state,
                    connection,
                    "commands.bossbar.create.failed",
                    vec![text(id)],
                );
                return;
            }
            let parsed = match crate::command_text::parse_component(name) {
                Ok(tag) => tag,
                Err(error) => {
                    error_text(state, connection, error.message());
                    return;
                }
            };
            let resolved = match crate::command_text::resolve_component(state, executor, &parsed) {
                Ok(tag) => tag,
                Err(message) => {
                    error_text(state, connection, message);
                    return;
                }
            };
            state.bossbars.bars.insert(
                id.clone(),
                BossBar {
                    id: id.clone(),
                    name: resolved,
                    value: 0,
                    max: 100,
                    color: BarColor::White,
                    overlay: BarOverlay::Progress,
                    visible: true,
                    players: BTreeSet::new(),
                },
            );
            state.bossbars_dirty = true;
            let display = state.bossbars.bars[id].name.clone();
            feedback(
                state,
                connection,
                "commands.bossbar.create.success",
                vec![display],
            );
        }
        Cmd::Remove { id } => {
            let Some(bar) = state.bossbars.bars.remove(id) else {
                unknown(state, connection, id);
                return;
            };
            send_to_members(state, &bar.players, bar.uuid(), &BossBarChange::Remove);
            state.bossbars_dirty = true;
            feedback(
                state,
                connection,
                "commands.bossbar.remove.success",
                vec![bar.name],
            );
        }
        Cmd::List => {
            if state.bossbars.bars.is_empty() {
                feedback(
                    state,
                    connection,
                    "commands.bossbar.list.bars.none",
                    Vec::new(),
                );
            } else {
                let names: Vec<mc_nbt::Tag> = state
                    .bossbars
                    .bars
                    .values()
                    .map(|bar| bar.name.clone())
                    .collect();
                let count = names.len();
                feedback(
                    state,
                    connection,
                    "commands.bossbar.list.bars.some",
                    vec![text(count), format_list(names)],
                );
            }
        }
        Cmd::SetName { id, name } => {
            let parsed = match crate::command_text::parse_component(name) {
                Ok(tag) => tag,
                Err(error) => {
                    error_text(state, connection, error.message());
                    return;
                }
            };
            let resolved = match crate::command_text::resolve_component(state, executor, &parsed) {
                Ok(tag) => tag,
                Err(message) => {
                    error_text(state, connection, message);
                    return;
                }
            };
            let unchanged = state
                .bossbars
                .bars
                .get(id)
                .is_some_and(|bar| bar.name == resolved);
            if !state.bossbars.bars.contains_key(id) {
                unknown(state, connection, id);
                return;
            }
            if unchanged {
                failure(state, connection, "commands.bossbar.set.name.unchanged");
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.name = resolved;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display) = (bar.players.clone(), bar.uuid(), bar.name.clone());
            let change = BossBarChange::Name(display.clone());
            send_to_members(state, &players, bar_id, &change);
            feedback(
                state,
                connection,
                "commands.bossbar.set.name.success",
                vec![display],
            );
        }
        Cmd::SetColor { id, color } => {
            let Some(current) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if current.color == *color {
                failure(state, connection, "commands.bossbar.set.color.unchanged");
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.color = *color;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, style) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                (bar.color as i32, bar.overlay as i32),
            );
            let change = BossBarChange::Style {
                color: style.0,
                overlay: style.1,
            };
            send_to_members(state, &players, bar_id, &change);
            feedback(
                state,
                connection,
                "commands.bossbar.set.color.success",
                vec![display],
            );
        }
        Cmd::SetStyle { id, overlay } => {
            let Some(current) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if current.overlay == *overlay {
                failure(state, connection, "commands.bossbar.set.style.unchanged");
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.overlay = *overlay;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, style) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                (bar.color as i32, bar.overlay as i32),
            );
            let change = BossBarChange::Style {
                color: style.0,
                overlay: style.1,
            };
            send_to_members(state, &players, bar_id, &change);
            feedback(
                state,
                connection,
                "commands.bossbar.set.style.success",
                vec![display],
            );
        }
        Cmd::SetValue { id, value } => {
            let Some(current) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if current.value == *value {
                failure(state, connection, "commands.bossbar.set.value.unchanged");
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.value = *value;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, progress) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                bar.progress(),
            );
            let change = BossBarChange::Progress(progress);
            send_to_members(state, &players, bar_id, &change);
            feedback(
                state,
                connection,
                "commands.bossbar.set.value.success",
                vec![display, text(*value)],
            );
        }
        Cmd::SetMax { id, max } => {
            let Some(current) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if current.max == *max {
                failure(state, connection, "commands.bossbar.set.max.unchanged");
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.max = *max;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, progress) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                bar.progress(),
            );
            let change = BossBarChange::Progress(progress);
            send_to_members(state, &players, bar_id, &change);
            feedback(
                state,
                connection,
                "commands.bossbar.set.max.success",
                vec![display, text(*max)],
            );
        }
        Cmd::SetVisible { id, visible } => {
            let Some(current) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if current.visible == *visible {
                failure(
                    state,
                    connection,
                    if *visible {
                        "commands.bossbar.set.visibility.unchanged.visible"
                    } else {
                        "commands.bossbar.set.visibility.unchanged.hidden"
                    },
                );
                return;
            }
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.visible = *visible;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, snapshot) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                (bar.progress(), bar.color as i32, bar.overlay as i32),
            );
            if *visible {
                let add = BossBarChange::Add {
                    name: display.clone(),
                    progress: snapshot.0,
                    color: snapshot.1,
                    overlay: snapshot.2,
                    flags: 0,
                };
                send_to_members(state, &players, bar_id, &add);
                feedback(
                    state,
                    connection,
                    "commands.bossbar.set.visible.success.visible",
                    vec![display],
                );
            } else {
                send_to_members(state, &players, bar_id, &BossBarChange::Remove);
                feedback(
                    state,
                    connection,
                    "commands.bossbar.set.visible.success.hidden",
                    vec![display],
                );
            }
        }
        Cmd::SetPlayers { id, targets } => {
            if !state.bossbars.bars.contains_key(id) {
                unknown(state, connection, id);
                return;
            }
            let wanted: BTreeSet<Uuid> = match targets.as_deref() {
                None => BTreeSet::new(),
                Some(word) => match resolve_member_uuids(state, executor, word) {
                    Ok(uuids) => uuids.into_iter().collect(),
                    Err(key) => {
                        feedback(state, connection, key, Vec::new());
                        return;
                    }
                },
            };
            let current = state
                .bossbars
                .bars
                .get(id)
                .expect("checked bar")
                .players
                .clone();
            if current == wanted {
                failure(state, connection, "commands.bossbar.set.players.unchanged");
                return;
            }
            let removed: Vec<Uuid> = current.difference(&wanted).copied().collect();
            let added: Vec<Uuid> = wanted.difference(&current).copied().collect();
            {
                let bar = state.bossbars.bars.get_mut(id).expect("checked bar");
                bar.players = wanted;
            }
            state.bossbars_dirty = true;
            let bar = state.bossbars.bars.get(id).expect("checked bar");
            let (players, bar_id, display, visible, snapshot) = (
                bar.players.clone(),
                bar.uuid(),
                bar.name.clone(),
                bar.visible,
                (bar.progress(), bar.color as i32, bar.overlay as i32),
            );
            let progress = snapshot.0;
            let color = snapshot.1;
            let overlay = snapshot.2;
            if visible {
                for uuid in removed {
                    let single: BTreeSet<Uuid> = [uuid].into_iter().collect();
                    send_to_members(state, &single, bar_id, &BossBarChange::Remove);
                }
                let add = BossBarChange::Add {
                    name: display.clone(),
                    progress,
                    color,
                    overlay,
                    flags: 0,
                };
                for uuid in added {
                    let single: BTreeSet<Uuid> = [uuid].into_iter().collect();
                    send_to_members(state, &single, bar_id, &add);
                }
            } else {
                // Hidden bars hold no client-side state; still drop removed
                // members that were visible before hiding.
                for uuid in removed {
                    let single: BTreeSet<Uuid> = [uuid].into_iter().collect();
                    send_to_members(state, &single, bar_id, &BossBarChange::Remove);
                }
            }
            if players.is_empty() {
                feedback(
                    state,
                    connection,
                    "commands.bossbar.set.players.success.none",
                    vec![display],
                );
            } else {
                let names = member_names(state, &players.iter().copied().collect::<Vec<_>>());
                feedback(
                    state,
                    connection,
                    "commands.bossbar.set.players.success.some",
                    vec![display, text(players.len()), format_list(names)],
                );
            }
        }
        Cmd::GetValue { id } => {
            let Some(bar) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            feedback(
                state,
                connection,
                "commands.bossbar.get.value",
                vec![bar.name.clone(), text(bar.value)],
            );
        }
        Cmd::GetMax { id } => {
            let Some(bar) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            feedback(
                state,
                connection,
                "commands.bossbar.get.max",
                vec![bar.name.clone(), text(bar.max)],
            );
        }
        Cmd::GetVisible { id } => {
            let Some(bar) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            feedback(
                state,
                connection,
                if bar.visible {
                    "commands.bossbar.get.visible.visible"
                } else {
                    "commands.bossbar.get.visible.hidden"
                },
                vec![bar.name.clone()],
            );
        }
        Cmd::GetPlayers { id } => {
            let Some(bar) = state.bossbars.bars.get(id) else {
                unknown(state, connection, id);
                return;
            };
            if bar.players.is_empty() {
                feedback(
                    state,
                    connection,
                    "commands.bossbar.get.players.none",
                    vec![bar.name.clone()],
                );
            } else {
                let names = member_names(state, &bar.players.iter().copied().collect::<Vec<_>>());
                feedback(
                    state,
                    connection,
                    "commands.bossbar.get.players.some",
                    vec![
                        bar.name.clone(),
                        text(bar.players.len()),
                        format_list(names),
                    ],
                );
            }
        }
    }
}

/// Re-send visible bars to a joining connection (`sync_join` hook, next to
/// `scoreboard::sync_join`): vanilla `CustomBossEvents.onPlayerConnect`.
pub(crate) fn sync_join(state: &mut TickState, connection: ConnectionId) {
    let Some(uuid) = state.connections.uuid_of(connection) else {
        return;
    };
    let adds: Vec<(Uuid, BossBarChange)> = state
        .bossbars
        .bars
        .values()
        .filter(|bar| bar.visible && bar.players.contains(&uuid))
        .map(|bar| (bar.uuid(), add_packet(bar)))
        .collect();
    for (id, change) in adds {
        state.push_player(connection, PlayerChange::BossBar { id, change });
    }
}

/// Re-send visible bars after a dimension change or respawn. Both rebuild
/// the client level (edge `plan_respawn` drops `boss_baseline`), so the
/// client drops its bars with it; `Add` is idempotent.
pub(crate) fn sync_dimension(state: &mut TickState, connection: ConnectionId) {
    sync_join(state, connection);
}

#[cfg(test)]
#[path = "bossbar_tests.rs"]
mod tests;
