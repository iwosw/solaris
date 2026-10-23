//! Sculk sensors and catalyst spreading, retained through opaque vanilla BE post-images.
use crate::boundary::{BlockPos, ChunkPos};
use crate::tick::TickState;
use crate::vibration::{self, GameEvent};
use mc_data::item_nbt::field;
use mc_nbt::{ListTag, Tag};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub(crate) struct SculkState {
    loaded: HashSet<ChunkPos>,
    /// World residency stamp of the last completed hydration pass.
    hydrated: Option<(u64, usize)>,
    entries: HashMap<BlockPos, Entry>,
}
#[derive(Clone, Debug, PartialEq)]
enum Entry {
    Sensor {
        frequency: u8,
        deadline: u64,
    },
    Catalyst {
        cursors: Vec<Cursor>,
        bloom_until: u64,
    },
}
#[derive(Clone, Debug, PartialEq)]
struct Cursor {
    pos: BlockPos,
    charge: i32,
    decay_delay: i32,
    update_delay: i32,
    facings: Option<Vec<String>>,
}
fn block(state: &TickState, pos: BlockPos) -> Option<&mc_world::BlockState> {
    state.registry.blocks.by_id(mc_world::BlockStateId(
        state.world.block_at(pos.x, pos.y, pos.z)?,
    ))
}
fn property<'a>(b: &'a mc_world::BlockState, name: &str) -> Option<&'a str> {
    b.properties
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}
fn sculk(path: &str) -> bool {
    matches!(
        path,
        "sculk_sensor" | "calibrated_sculk_sensor" | "sculk_catalyst"
    )
}
fn base_fields(pos: BlockPos, name: &str) -> Vec<(String, Tag)> {
    vec![
        ("id".into(), Tag::String(format!("minecraft:{name}"))),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
    ]
}
fn list(elements: Vec<Tag>, kind: u8) -> Tag {
    Tag::List(ListTag {
        element_type: kind,
        elements,
    })
}
fn int(fields: &[(String, Tag)], name: &str, default: i32, max: i32) -> Result<i32, String> {
    match field(fields, name) {
        None => Ok(default),
        Some(Tag::Int(v)) if (0..=max).contains(v) => Ok(*v),
        _ => Err(format!("{name} must be an int in 0..={max}")),
    }
}
fn deadline(fields: &[(String, Tag)], name: &str) -> Result<u64, String> {
    match field(fields, name) {
        None => Ok(0),
        Some(Tag::Long(v)) if *v >= 0 => Ok(*v as u64),
        _ => Err(format!("{name} must be a nonnegative long")),
    }
}
fn decode(bytes: &[u8], tick: u64) -> Result<(Entry, Option<vibration::Pending>), String> {
    let mut bytes = bytes;
    let Tag::Compound(f) =
        mc_nbt::read_network(&mut bytes).map_err(|e| format!("invalid NBT: {e}"))?
    else {
        return Err("root must be a compound".into());
    };
    if !bytes.is_empty() {
        return Err("trailing NBT bytes".into());
    }
    let name = match field(&f, "id") {
        Some(Tag::String(s)) => s.as_str(),
        _ => return Err("id must be a string".into()),
    };
    match name {
        "minecraft:sculk_sensor" | "minecraft:calibrated_sculk_sensor" => {
            let frequency = int(&f, "last_vibration_frequency", 0, 15)? as u8;
            let mut pending = field(&f, "listener")
                .map(|v| vibration::decode_listener(v, tick))
                .transpose()?
                .flatten();
            if let Some(p) = &mut pending
                && field(&f, "solaris:event_arrival").is_some()
            {
                p.arrival = deadline(&f, "solaris:event_arrival")?;
            }
            Ok((
                Entry::Sensor {
                    frequency,
                    deadline: deadline(&f, "solaris:phase_until")?,
                },
                pending,
            ))
        }
        "minecraft:sculk_catalyst" => {
            let mut cursors = Vec::new();
            if let Some(tag) = field(&f, "cursors") {
                let Tag::List(l) = tag else {
                    return Err("cursors must be a compound list".into());
                };
                if l.elements.len() > 32 {
                    return Err("cursors exceeds 32 entries".into());
                }
                for tag in &l.elements {
                    let Tag::Compound(f) = tag else {
                        return Err("cursors must be a compound list".into());
                    };
                    let pos = match field(f, "pos") {
                        Some(Tag::IntArray(p)) if p.len() == 3 => BlockPos {
                            x: p[0],
                            y: p[1],
                            z: p[2],
                        },
                        _ => return Err("cursor.pos must be a three-int array".into()),
                    };
                    let facings =
                        match field(f, "facings") {
                            None => None,
                            Some(Tag::List(l)) => Some(
                                l.elements
                                    .iter()
                                    .map(|t| match t {
                                        Tag::String(v)
                                            if matches!(
                                                v.as_str(),
                                                "down" | "up" | "north" | "south" | "west" | "east"
                                            ) =>
                                        {
                                            Ok(v.clone())
                                        }
                                        _ => Err("cursor.facings must contain direction strings"
                                            .to_owned()),
                                    })
                                    .collect::<Result<Vec<_>, _>>()?,
                            ),
                            _ => return Err("cursor.facings must be a string list".into()),
                        };
                    cursors.push(Cursor {
                        pos,
                        charge: int(f, "charge", 0, 1000)?,
                        decay_delay: int(f, "decay_delay", 1, 1)?,
                        update_delay: int(f, "update_delay", 0, i32::MAX)?,
                        facings,
                    });
                }
            }
            Ok((
                Entry::Catalyst {
                    cursors,
                    bloom_until: deadline(&f, "solaris:bloom_until")?,
                },
                None,
            ))
        }
        _ => Err("id is not a sculk block entity".into()),
    }
}
fn encode(state: &TickState, pos: BlockPos, entry: &Entry) -> Vec<u8> {
    let name = block(state, pos)
        .map(|b| b.block.id.path())
        .unwrap_or("sculk_sensor");
    let mut f = base_fields(pos, name);
    match entry {
        Entry::Sensor {
            frequency,
            deadline,
        } => {
            f.push((
                "last_vibration_frequency".into(),
                Tag::Int(i32::from(*frequency)),
            ));
            f.push((
                "listener".into(),
                vibration::listener_nbt(vibration::pending(state, pos), state.tick),
            ));
            if let Some(p) = vibration::pending(state, pos) {
                f.push(("solaris:event_arrival".into(), Tag::Long(p.arrival as i64)));
            }
            if *deadline != 0 {
                f.push(("solaris:phase_until".into(), Tag::Long(*deadline as i64)));
            }
        }
        Entry::Catalyst {
            cursors,
            bloom_until,
        } => {
            let entries = cursors
                .iter()
                .map(|c| {
                    let mut f = vec![
                        ("pos".into(), Tag::IntArray(vec![c.pos.x, c.pos.y, c.pos.z])),
                        ("charge".into(), Tag::Int(c.charge)),
                        ("decay_delay".into(), Tag::Int(c.decay_delay)),
                        ("update_delay".into(), Tag::Int(c.update_delay)),
                    ];
                    if let Some(facings) = &c.facings {
                        f.push((
                            "facings".into(),
                            list(
                                facings.iter().cloned().map(Tag::String).collect(),
                                mc_nbt::tag_type::STRING,
                            ),
                        ));
                    }
                    Tag::Compound(f)
                })
                .collect();
            f.push(("cursors".into(), list(entries, mc_nbt::tag_type::COMPOUND)));
            if *bloom_until != 0 {
                f.push(("solaris:bloom_until".into(), Tag::Long(*bloom_until as i64)));
            }
        }
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(f)).expect("sculk NBT");
    bytes
}

pub(crate) fn persist_listener(state: &mut TickState, pos: BlockPos) -> bool {
    let Some(entry) = state.sculk.entries.remove(&pos) else {
        return block(state, pos).is_none_or(|b| {
            !matches!(
                b.block.id.path(),
                "sculk_sensor" | "calibrated_sculk_sensor"
            )
        });
    };
    if matches!(entry, Entry::Sensor { .. }) {
        persist(state, pos, &entry);
    }
    state.sculk.entries.insert(pos, entry);
    true
}

pub(crate) fn removed(state: &mut TickState, pos: BlockPos) {
    crate::shrieker::removed(state, pos);
    state.sculk.entries.remove(&pos);
    vibration::unlisten(state, pos);
}

fn has_image(state: &TickState, pos: BlockPos) -> bool {
    state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .is_some_and(|c| c.block_entities.contains_key(&pos))
}

fn persist(state: &mut TickState, pos: BlockPos, entry: &Entry) {
    let bytes = encode(state, pos, entry);
    let cp = ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    if state
        .world
        .chunk(cp)
        .and_then(|c| c.block_entities.get(&pos))
        .is_some_and(|b| *b == bytes)
    {
        return;
    }
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}

/// Region captures use vanilla remaining travel time; WAL images retain the
/// absolute arrival fence and need no rewrite during quiet in-flight ticks.
pub(crate) fn checkpoint(state: &mut TickState) {
    let entries = std::mem::take(&mut state.sculk.entries);
    for (pos, entry) in &entries {
        if block(state, *pos).is_some_and(|b| sculk(b.block.id.path())) {
            let bytes = encode(state, *pos, entry);
            state.world.set_opaque_block_entity(*pos, Some(bytes));
        }
    }
    state.sculk.entries = entries;
}
/// Validate retained records before adopting a replay/checkpoint world.
pub(crate) fn validate(state: &TickState) -> Result<(), String> {
    for chunk in state.world.chunks() {
        for (pos, bytes) in &chunk.block_entities {
            if block(state, *pos).is_some_and(|b| sculk(b.block.id.path())) {
                decode(bytes, state.tick).map_err(|e| {
                    format!("sculk block entity at {},{},{}: {e}", pos.x, pos.y, pos.z)
                })?;
            }
        }
    }
    Ok(())
}
pub(crate) fn validate_record(bytes: &[u8]) -> Result<(), String> {
    let mut input = bytes;
    let Tag::Compound(f) = mc_nbt::read_network(&mut input).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if matches!(field(&f,"id"),Some(Tag::String(n)) if matches!(n.as_str(),"minecraft:sculk_sensor"|"minecraft:calibrated_sculk_sensor"|"minecraft:sculk_catalyst"))
    {
        decode(bytes, 0)?;
    }
    Ok(())
}
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    crate::shrieker::changed(state, pos);
    let Some(b) = block(state, pos) else {
        return;
    };
    if !sculk(b.block.id.path()) {
        if state.sculk.entries.remove(&pos).is_some() {
            vibration::unlisten(state, pos);
        }
        return;
    }
    let catalyst = b.block.id.path() == "sculk_catalyst";
    let calibrated = b.block.id.path() == "calibrated_sculk_sensor";
    if state
        .sculk
        .entries
        .get(&pos)
        .is_some_and(|entry| matches!(entry, Entry::Catalyst { .. }) == catalyst)
        && has_image(state, pos)
    {
        if !catalyst {
            vibration::listen(
                state,
                pos,
                vibration::centre(pos),
                if calibrated { 16 } else { 8 },
            );
        }
        return;
    }
    removed(state, pos);
    let cp = ChunkPos {
        x: pos.x.div_euclid(16),
        z: pos.z.div_euclid(16),
    };
    let stored = state
        .world
        .chunk(cp)
        .and_then(|c| c.block_entities.get(&pos))
        .map(|b| {
            decode(b, state.tick).unwrap_or_else(|e| {
                panic!("sculk block entity at {},{},{}: {e}", pos.x, pos.y, pos.z)
            })
        });
    let (entry, pending) = stored.unwrap_or_else(|| {
        (
            if catalyst {
                Entry::Catalyst {
                    cursors: Vec::new(),
                    bloom_until: 0,
                }
            } else {
                Entry::Sensor {
                    frequency: 0,
                    deadline: 0,
                }
            },
            None,
        )
    });
    if !catalyst {
        vibration::listen(
            state,
            pos,
            vibration::centre(pos),
            if calibrated { 16 } else { 8 },
        );
        vibration::restore(state, pos, pending);
    }
    state.sculk.entries.insert(pos, entry);
}
pub(crate) fn hydrate(state: &mut TickState) {
    vibration::evict_unloaded(state);
    state.sculk.entries.retain(|pos, _| {
        state
            .world
            .has_chunk(pos.x.div_euclid(16), pos.z.div_euclid(16))
    });
    // `active_revision` advances on every resident insert/remove, so an
    // unchanged stamp means `loaded` already holds every resident chunk.
    let stamp = (state.world.active_revision(), state.world.resident_count());
    if state.sculk.hydrated == Some(stamp) {
        return;
    }
    let mut resident = state.world.resident_positions();
    resident.sort_unstable_by_key(|p| (p.x, p.z));
    state
        .sculk
        .loaded
        .retain(|p| state.world.has_chunk(p.x, p.z));
    for cp in resident {
        if !state.sculk.loaded.insert(cp) {
            continue;
        }
        let Some(chunk) = state.world.chunk(cp) else {
            continue;
        };
        let mut cells = Vec::new();
        for (index, s) in chunk.sections.iter().enumerate() {
            if s.is_empty() {
                continue;
            }
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        if state
                            .registry
                            .blocks
                            .by_id(s.get(x, y, z))
                            .is_some_and(|b| sculk(b.block.id.path()))
                        {
                            cells.push(BlockPos {
                                x: cp.x * 16 + i32::from(x),
                                y: chunk.geometry().min_y() + index as i32 * 16 + i32::from(y),
                                z: cp.z * 16 + i32::from(z),
                            });
                        }
                    }
                }
            }
        }
        for pos in cells {
            changed(state, pos);
        }
    }
    state.sculk.hydrated = Some(stamp);
}
pub(crate) fn accepts(state: &TickState, pos: BlockPos, frequency: u8, event: GameEvent) -> bool {
    let Some(b) = block(state, pos) else {
        return true;
    };
    let path = b.block.id.path();
    if path == "sculk_shrieker" {
        return event == GameEvent::TendrilsClicking;
    }
    if !matches!(path, "sculk_sensor" | "calibrated_sculk_sensor") {
        return true;
    }
    if property(b, "sculk_sensor_phase") != Some("inactive") {
        return false;
    }
    if frequency == 0 {
        return false;
    }
    if path == "calibrated_sculk_sensor" {
        let (dx, dz) = match property(b, "facing") {
            Some("north") => (0, 1),
            Some("south") => (0, -1),
            Some("west") => (1, 0),
            _ => (-1, 0),
        };
        let back = BlockPos {
            x: pos.x + dx,
            z: pos.z + dz,
            ..pos
        };
        let input = crate::redstone::output(state, back, pos);
        if input != 0 && input != frequency {
            return false;
        }
    }
    true
}
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> Option<u8> {
    let b = block(state, pos)?;
    if !matches!(
        b.block.id.path(),
        "sculk_sensor" | "calibrated_sculk_sensor"
    ) {
        return None;
    }
    if property(b, "sculk_sensor_phase") != Some("active") {
        return Some(0);
    }
    if let Some(Entry::Sensor { frequency, .. }) = state.sculk.entries.get(&pos) {
        return Some(*frequency);
    }
    Some(0)
}

pub(crate) fn signal(state: &TickState, source: BlockPos, target: BlockPos) -> u8 {
    let Some(b) = block(state, source) else {
        return 0;
    };
    if b.block.id.path() == "calibrated_sculk_sensor" {
        let (dx, dz) = match property(b, "facing") {
            Some("north") => (0, 1),
            Some("south") => (0, -1),
            Some("west") => (1, 0),
            _ => (-1, 0),
        };
        if target
            == (BlockPos {
                x: source.x + dx,
                z: source.z + dz,
                ..source
            })
        {
            return 0;
        }
    }
    property(b, "power")
        .and_then(|v| v.parse().ok())
        .unwrap_or(0)
}
fn change(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    pos: BlockPos,
    props: &[(&str, String)],
) {
    let Some(b) = block(state, pos) else {
        return;
    };
    let mut values = b.properties.clone();
    for (key, value) in props {
        if let Some((_, v)) = values.iter_mut().find(|(k, _)| k == key) {
            *v = value.clone();
        }
    }
    let new = state
        .registry
        .blocks
        .by_name_and_props(&b.block.id, &values);
    if let Some(new) = new {
        crate::redstone::set(state, writes, pos, new.0);
    }
}
/// First loaded catalyst within eight blocks owns the XP, once at death commit.
pub(crate) fn absorb(state: &mut TickState, pos: [f64; 3], xp: i32) -> bool {
    hydrate(state);
    let key = state
        .sculk
        .entries
        .keys()
        .copied()
        .filter(|p| {
            block(state, *p).is_some_and(|b| b.block.id.path() == "sculk_catalyst")
                && vibration::distance(vibration::centre(*p), pos) <= 8.0
        })
        .min_by(|a, b| {
            vibration::distance(vibration::centre(*a), pos)
                .total_cmp(&vibration::distance(vibration::centre(*b), pos))
                .then_with(|| (a.x, a.y, a.z).cmp(&(b.x, b.y, b.z)))
        });
    let Some(key) = key else {
        return false;
    };
    let Some(mut entry) = state.sculk.entries.remove(&key) else {
        return false;
    };
    if let Entry::Catalyst {
        cursors,
        bloom_until,
    } = &mut entry
    {
        *bloom_until = state.tick + 8;
        let at = BlockPos {
            x: pos[0].floor() as i32,
            y: (pos[1] - 0.5).floor() as i32,
            z: pos[2].floor() as i32,
        };
        let mut charge = xp.max(0);
        for cursor in cursors.iter_mut().filter(|c| c.pos == at) {
            let add = charge.min(1000 - cursor.charge);
            cursor.charge += add;
            charge -= add;
        }
        while charge > 0 && cursors.len() < 32 {
            let add = charge.min(1000);
            cursors.push(Cursor {
                pos: at,
                charge: add,
                decay_delay: 1,
                update_delay: 0,
                facings: None,
            });
            charge -= add;
        }
    }
    persist(state, key, &entry);
    state.sculk.entries.insert(key, entry);
    true
}

pub(crate) fn death(state: &mut TickState, slot: crate::entities::EntitySlot) -> Option<i32> {
    let row = state.entities.row(slot);
    let crate::entities::EntityKind::Mob { type_id } = state.entities.body(slot).kind else {
        return None;
    };
    let xp = state.registry.blueprint(type_id).map_or(0, |b| {
        crate::entity_loot::death_experience(
            &b.name,
            row,
            state.tick ^ state.entities.body(slot).id.0 as u64,
        )
    });
    let id = state.entities.body(slot).id;
    let pos = state.entities.pos(slot);
    vibration::emit(state, pos, GameEvent::EntityDie, Some(id));
    absorb(state, pos, xp).then_some(xp)
}
fn spread(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, cursor: &mut Cursor) {
    if cursor.update_delay > 0 {
        cursor.update_delay -= 1;
        return;
    }
    let sculk_id = state
        .registry
        .blocks
        .block(&mc_data::Identifier::parse("minecraft:sculk").expect("id"))
        .expect("sculk block")
        .default
        .0;
    for (dx, dz) in [(0, 0), (1, 0), (-1, 0), (0, 1), (0, -1)] {
        let at = BlockPos {
            x: cursor.pos.x + dx,
            z: cursor.pos.z + dz,
            ..cursor.pos
        };
        let Some(b) = block(state, at) else {
            continue;
        };
        if state.registry.tags.contains_raw_id(
            "minecraft:block",
            "minecraft:sculk_replaceable",
            b.block.raw_id,
        ) {
            crate::redstone::set(state, writes, at, sculk_id);
            let above = BlockPos { y: at.y + 1, ..at };
            if state.world.block_at(above.x, above.y, above.z) == Some(state.registry.states.air) {
                let id = mc_data::Identifier::parse("minecraft:sculk_vein").expect("id");
                let b = state.registry.blocks.block(&id).expect("vein");
                let mut props = state
                    .registry
                    .blocks
                    .by_id(b.default)
                    .expect("vein state")
                    .properties
                    .clone();
                if let Some((_, v)) = props.iter_mut().find(|(k, _)| k == "down") {
                    *v = "true".into();
                }
                if let Some(vein) = state.registry.blocks.by_name_and_props(&id, &props) {
                    crate::redstone::set(state, writes, above, vein.0);
                }
            }
            cursor.pos = at;
            cursor.charge -= 1;
            cursor.update_delay = 1;
            return;
        }
    }
    cursor.charge = 0;
}
pub(crate) fn run(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>) {
    hydrate(state);
    let incoming: Vec<_> = writes.iter().map(|(p, _)| *p).collect();
    for pos in incoming {
        changed(state, pos);
    }
    let mut entries = std::mem::take(&mut state.sculk.entries);
    entries.retain(|pos, entry| {
        let Some(b) = block(state, *pos) else {
            return true;
        };
        if !state
            .world
            .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            return true;
        }
        if !sculk(b.block.id.path()) {
            vibration::unlisten(state, *pos);
            return false;
        }
        match entry {
            Entry::Sensor {
                frequency,
                deadline,
            } => {
                let phase = property(b, "sculk_sensor_phase").unwrap_or("inactive");
                let calibrated = b.block.id.path() == "calibrated_sculk_sensor";
                if phase == "inactive"
                    && has_image(state, *pos)
                    && vibration::pending(state, *pos).is_some_and(|p| state.tick < p.arrival)
                {
                    return true;
                }
                if phase == "inactive"
                    && vibration::pending(state, *pos).is_none()
                    && *deadline == 0
                    && has_image(state, *pos)
                {
                    return true;
                }
                if phase != "inactive" && (*deadline == 0 || state.tick >= *deadline) {
                    if phase == "active" {
                        change(
                            state,
                            writes,
                            *pos,
                            &[
                                ("sculk_sensor_phase", "cooldown".into()),
                                ("power", "0".into()),
                            ],
                        );
                        *deadline = state.tick + 10;
                    } else {
                        change(
                            state,
                            writes,
                            *pos,
                            &[("sculk_sensor_phase", "inactive".into())],
                        );
                        *deadline = 0;
                    }
                } else if phase == "inactive"
                    && let Some(v) = vibration::take(state, *pos)
                {
                    *frequency = v.frequency;
                    *deadline = state.tick + if calibrated { 10 } else { 30 };
                    let power = (15
                        - (15.0 * v.distance / if calibrated { 16.0 } else { 8.0 }).floor() as i32)
                        .clamp(1, 15);
                    change(
                        state,
                        writes,
                        *pos,
                        &[
                            ("sculk_sensor_phase", "active".into()),
                            ("power", power.to_string()),
                        ],
                    );
                    vibration::emit(
                        state,
                        vibration::centre(*pos),
                        GameEvent::TendrilsClicking,
                        v.source,
                    );
                    // Amethyst adjacent to the sensor retransmits the received frequency.
                    for (dx, dy, dz) in [
                        (1, 0, 0),
                        (-1, 0, 0),
                        (0, 1, 0),
                        (0, -1, 0),
                        (0, 0, 1),
                        (0, 0, -1),
                    ] {
                        let at = BlockPos {
                            x: pos.x + dx,
                            y: pos.y + dy,
                            z: pos.z + dz,
                        };
                        if block(state, at).is_some_and(|b| {
                            state.registry.tags.contains_raw_id(
                                "minecraft:block",
                                "minecraft:vibration_resonators",
                                b.block.raw_id,
                            )
                        }) {
                            vibration::emit(
                                state,
                                vibration::centre(at),
                                GameEvent::Resonate(v.frequency),
                                v.source,
                            );
                        }
                    }
                }
            }
            Entry::Catalyst {
                cursors,
                bloom_until,
            } => {
                let blooming = property(b, "bloom") == Some("true");
                let desired = state.tick < *bloom_until;
                if !blooming && !desired && cursors.is_empty() && has_image(state, *pos) {
                    return true;
                }
                if blooming != desired {
                    change(state, writes, *pos, &[("bloom", desired.to_string())]);
                }
                for cursor in cursors.iter_mut() {
                    spread(state, writes, cursor);
                }
                cursors.retain(|c| c.charge > 0);
                if !desired {
                    *bloom_until = 0;
                }
            }
        }
        persist(state, *pos, entry);
        true
    });
    state.sculk.entries = entries;
    vibration::flush(state);
}
#[cfg(test)]
#[path = "sculk_tests.rs"]
mod tests;
