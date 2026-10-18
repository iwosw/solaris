//! Sculk shrieks and vanilla per-player warden_spawn_tracker persistence.
use crate::boundary::{BlockPos, ChunkPos, EntityId, PlayerChange};
use crate::tick::TickState;
use crate::vibration::{self, GameEvent, Pending};
use mc_nbt::Tag;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};

/// Durable warning values at `base_tick`; only activity and shrieks mutate them.
/// The read cursor is transient, so idle ticks do not create player postimages.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields, try_from = "WarningImage")]
pub struct Warning {
    ticks_since_last_warning: u32,
    warning_level: u32,
    cooldown_ticks: u32,
    base_tick: u64,
    active: bool,
    #[serde(skip)]
    current_tick: u64,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WarningImage {
    ticks_since_last_warning: u32,
    warning_level: u32,
    cooldown_ticks: u32,
    #[serde(default)]
    base_tick: u64,
    #[serde(default)]
    active: bool,
}
impl TryFrom<WarningImage> for Warning {
    type Error = String;
    fn try_from(value: WarningImage) -> Result<Self, String> {
        let warning = Self {
            ticks_since_last_warning: value.ticks_since_last_warning,
            warning_level: value.warning_level,
            cooldown_ticks: value.cooldown_ticks,
            base_tick: value.base_tick,
            active: value.active,
            current_tick: value.base_tick,
        };
        warning.validate()?;
        Ok(warning)
    }
}
impl PartialEq for Warning {
    fn eq(&self, other: &Self) -> bool {
        self.ticks_since_last_warning == other.ticks_since_last_warning
            && self.warning_level == other.warning_level
            && self.cooldown_ticks == other.cooldown_ticks
            && self.base_tick == other.base_tick
            && self.active == other.active
    }
}
impl Eq for Warning {}
impl Warning {
    fn validate(self) -> Result<(), String> {
        if self.warning_level > 4 {
            return Err("warden_spawn_tracker.warning_level: expected 0..=4".into());
        }
        if self.ticks_since_last_warning > i32::MAX as u32 {
            return Err(
                "warden_spawn_tracker.ticks_since_last_warning: expected nonnegative int".into(),
            );
        }
        if self.cooldown_ticks > 200 {
            return Err("warden_spawn_tracker.cooldown_ticks: expected 0..=200".into());
        }
        Ok(())
    }
    fn values_at(self, tick: u64) -> (u32, u32, u32) {
        let elapsed = if self.active {
            tick.saturating_sub(self.base_tick)
        } else {
            0
        };
        // Vanilla checks >= 12000 before incrementing: the reset tick is
        // the 12001st tick, including when warning level is already zero.
        let until_reset = 12001_u64
            .saturating_sub(u64::from(self.ticks_since_last_warning))
            .max(1);
        let (ticks, decays) = if elapsed == 0 || elapsed < until_reset {
            (u64::from(self.ticks_since_last_warning) + elapsed, 0)
        } else {
            let remaining = elapsed - until_reset;
            (remaining % 12001, 1 + remaining / 12001)
        };
        (
            ticks as u32,
            self.warning_level.saturating_sub(decays.min(4) as u32),
            self.cooldown_ticks.saturating_sub(elapsed.min(200) as u32),
        )
    }
    pub fn ticks_since_last_warning(self) -> u32 {
        self.values_at(self.current_tick).0
    }
    pub fn warning_level(self) -> u32 {
        self.values_at(self.current_tick).1
    }
    pub fn cooldown_ticks(self) -> u32 {
        self.values_at(self.current_tick).2
    }
    pub(crate) fn advance_tick(&mut self, tick: u64) {
        self.current_tick = tick;
    }
    pub(crate) fn set_active(&mut self, active: bool) {
        if self.active == active {
            return;
        }
        (
            self.ticks_since_last_warning,
            self.warning_level,
            self.cooldown_ticks,
        ) = self.values_at(self.current_tick);
        self.base_tick = self.current_tick;
        self.active = active;
    }
    pub(crate) fn freeze_at_tick(&mut self, tick: u64) {
        self.advance_tick(tick);
        self.set_active(false);
    }
    fn warned(level: u32, tick: u64) -> Self {
        Self {
            warning_level: level,
            cooldown_ticks: 200,
            base_tick: tick,
            current_tick: tick,
            active: true,
            ..Self::default()
        }
    }
    pub(crate) fn nbt(self) -> Tag {
        let (ticks, level, cooldown) = self.values_at(self.current_tick);
        Tag::Compound(vec![
            ("ticks_since_last_warning".into(), Tag::Int(ticks as i32)),
            ("warning_level".into(), Tag::Int(level as i32)),
            ("cooldown_ticks".into(), Tag::Int(cooldown as i32)),
        ])
    }
    pub(crate) fn read(fields: &[(String, Tag)]) -> Result<Self, String> {
        let Some(tag) = crate::warden::unique(fields, "warden_spawn_tracker")? else {
            return Ok(Self::default());
        };
        let Tag::Compound(fields) = tag else {
            return Err("warden_spawn_tracker: expected compound".into());
        };
        let warning = Self {
            ticks_since_last_warning: integer(fields, "ticks_since_last_warning", i32::MAX as u32)?,
            warning_level: integer(fields, "warning_level", 4)?,
            cooldown_ticks: integer(fields, "cooldown_ticks", 200)?,
            ..Self::default()
        };
        warning.validate()?;
        Ok(warning)
    }
}
fn integer(fields: &[(String, Tag)], name: &str, max: u32) -> Result<u32, String> {
    match crate::warden::unique(fields, name)? {
        None => Ok(0),
        Some(Tag::Int(n)) if *n >= 0 && *n as u32 <= max => Ok(*n as u32),
        _ => Err(format!("{name}: expected int 0..={max}")),
    }
}
#[derive(Default)]
pub(crate) struct Shriekers {
    loaded: HashSet<ChunkPos>,
    /// World residency stamp of the last completed hydration pass.
    hydrated: Option<(u64, usize)>,
    entries: BTreeMap<(i32, i32, i32), Shrieker>,
    positions: Vec<(i32, i32, i32)>,
}
#[derive(Clone, Copy, Default)]
struct Shrieker {
    warning_level: u32,
    shriek_until: u64,
}
fn key(pos: BlockPos) -> (i32, i32, i32) {
    (pos.x, pos.y, pos.z)
}
fn block(state: &TickState, pos: BlockPos) -> Option<&mc_world::BlockState> {
    state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(
            state.world.block_at(pos.x, pos.y, pos.z)?,
        ))
        .filter(|b| b.block.id.path() == "sculk_shrieker")
}
fn property(state: &TickState, pos: BlockPos, name: &str) -> bool {
    block(state, pos).is_some_and(|b| b.properties.iter().any(|(k, v)| k == name && v == "true"))
}
fn decode(bytes: &[u8], tick: u64) -> Result<(Shrieker, Option<Pending>), String> {
    let mut input = bytes;
    let Tag::Compound(fields) = mc_nbt::read_network(&mut input).map_err(|e| e.to_string())? else {
        return Err("sculk_shrieker: expected compound".into());
    };
    if !input.is_empty() {
        return Err("sculk_shrieker: trailing NBT bytes".into());
    }
    if !matches!(crate::warden::unique(&fields,"id")?,Some(Tag::String(id)) if id=="minecraft:sculk_shrieker")
    {
        return Err("sculk_shrieker.id: expected minecraft:sculk_shrieker".into());
    }
    let warning_level = integer(&fields, "warning_level", 4)?;
    let shriek_until = match crate::warden::unique(&fields, "solaris:shriek_until")? {
        None => 0,
        Some(Tag::Long(n)) if *n >= 0 => *n as u64,
        _ => return Err("sculk_shrieker.solaris:shriek_until: expected nonnegative long".into()),
    };
    let mut pending = crate::warden::unique(&fields, "listener")?
        .map(|tag| vibration::decode_listener(tag, tick))
        .transpose()?
        .flatten();
    if let Some(tag) = crate::warden::unique(&fields, "solaris:event_arrival")? {
        let Tag::Long(n) = tag else {
            return Err("sculk_shrieker.solaris:event_arrival: expected nonnegative long".into());
        };
        if *n < 0 {
            return Err("sculk_shrieker.solaris:event_arrival: expected nonnegative long".into());
        }
        if let Some(p) = &mut pending {
            p.arrival = *n as u64;
        }
    }
    Ok((
        Shrieker {
            warning_level,
            shriek_until,
        },
        pending,
    ))
}
pub(crate) fn validate_record(bytes: &[u8]) -> Result<(), String> {
    let mut input = bytes;
    let Tag::Compound(fields) = mc_nbt::read_network(&mut input).map_err(|e| e.to_string())? else {
        return Ok(());
    };
    if matches!(crate::warden::unique(&fields,"id")?,Some(Tag::String(id)) if id=="minecraft:sculk_shrieker")
    {
        decode(bytes, 0)?;
    }
    Ok(())
}
pub(crate) fn validate_loaded(state: &TickState) -> Result<(), String> {
    for chunk in state.world.chunks() {
        for (pos, bytes) in &chunk.block_entities {
            if block(state, *pos).is_some() {
                decode(bytes, state.tick).map_err(|err| {
                    format!("sculk_shrieker at {},{},{}: {err}", pos.x, pos.y, pos.z)
                })?;
            } else {
                validate_record(bytes).map_err(|err| {
                    format!("sculk_shrieker at {},{},{}: {err}", pos.x, pos.y, pos.z)
                })?;
            }
        }
    }
    Ok(())
}
pub(crate) fn changed(state: &mut TickState, pos: BlockPos) {
    if block(state, pos).is_none() {
        if state.shriekers.entries.remove(&key(pos)).is_some() {
            vibration::unlisten(state, pos);
        }
        return;
    }
    if state.shriekers.entries.contains_key(&key(pos)) {
        return;
    }
    let saved = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|c| c.block_entities.get(&pos));
    let (mut entry, pending) = saved
        .map(|b| {
            decode(b, state.tick)
                .unwrap_or_else(|e| panic!("sculk_shrieker at {},{},{}: {e}", pos.x, pos.y, pos.z))
        })
        .unwrap_or_default();
    // Vanilla imports retain the scheduled block tick; native records retain
    // an absolute deadline so replay never repeats the warning/spawn response.
    if entry.shriek_until == 0 && property(state, pos, "shrieking") {
        entry.shriek_until = state.tick + 90;
    }
    state.shriekers.entries.insert(key(pos), entry);
    vibration::listen(state, pos, vibration::centre(pos), 8);
    vibration::restore(state, pos, pending);
    persist_listener(state, pos);
}
pub(crate) fn removed(state: &mut TickState, pos: BlockPos) {
    state.shriekers.entries.remove(&key(pos));
    vibration::unlisten(state, pos);
}
pub(crate) fn hydrate(state: &mut TickState) {
    // `active_revision` advances on every resident insert/remove, so an
    // unchanged stamp means `loaded` already holds every resident chunk.
    let stamp = (state.world.active_revision(), state.world.resident_count());
    if state.shriekers.hydrated == Some(stamp) {
        return;
    }
    state
        .shriekers
        .loaded
        .retain(|cp| state.world.has_chunk(cp.x, cp.z));
    let resident = state.world.resident_positions();
    for cp in resident {
        if !state.shriekers.loaded.insert(cp) {
            continue;
        }
        let Some(chunk) = state.world.chunk(cp) else {
            continue;
        };
        let mut cells = Vec::new();
        for (index, section) in chunk.sections.iter().enumerate() {
            if section.is_empty() {
                continue;
            }
            for y in 0..16 {
                for z in 0..16 {
                    for x in 0..16 {
                        if state
                            .registry
                            .blocks
                            .by_id(section.get(x, y, z))
                            .is_some_and(|b| b.block.id.path() == "sculk_shrieker")
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
    state.shriekers.hydrated = Some(stamp);
}
fn encode(state: &TickState, pos: BlockPos, entry: &Shrieker) -> Vec<u8> {
    let mut fields = vec![
        ("id".into(), Tag::String("minecraft:sculk_shrieker".into())),
        ("x".into(), Tag::Int(pos.x)),
        ("y".into(), Tag::Int(pos.y)),
        ("z".into(), Tag::Int(pos.z)),
        ("warning_level".into(), Tag::Int(entry.warning_level as i32)),
        (
            "listener".into(),
            vibration::listener_nbt(vibration::pending(state, pos), state.tick),
        ),
        (
            "solaris:shriek_until".into(),
            Tag::Long(entry.shriek_until as i64),
        ),
    ];
    if let Some(p) = vibration::pending(state, pos) {
        fields.push(("solaris:event_arrival".into(), Tag::Long(p.arrival as i64)));
    }
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("shrieker NBT");
    bytes
}
pub(crate) fn persist_listener(state: &mut TickState, pos: BlockPos) -> bool {
    let Some(entry) = state.shriekers.entries.get(&key(pos)) else {
        return false;
    };
    let bytes = encode(state, pos, entry);
    let old = state
        .world
        .chunk(ChunkPos {
            x: pos.x.div_euclid(16),
            z: pos.z.div_euclid(16),
        })
        .and_then(|c| c.block_entities.get(&pos));
    if old == Some(&bytes) {
        return true;
    }
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
    true
}
fn set_shrieking(state: &mut TickState, pos: BlockPos, value: bool) {
    let Some(current) = block(state, pos) else {
        return;
    };
    let Some(next) = current
        .block
        .states
        .iter()
        .filter_map(|id| state.registry.blocks.by_id(*id))
        .find(|candidate| {
            current.properties.iter().all(|(name, old)| {
                candidate.properties.iter().any(|(key, new)| {
                    key == name
                        && new
                            == if name == "shrieking" {
                                if value { "true" } else { "false" }
                            } else {
                                old
                            }
                })
            })
        })
        .map(|candidate| candidate.id)
    else {
        return;
    };
    if state.world.block_at(pos.x, pos.y, pos.z) != Some(next.0) {
        state.world.set_block(pos.x, pos.y, pos.z, next.0);
        state.merge_block_writes.push((pos, next.0));
    }
}
fn nearby_warden(state: &TickState, pos: [f64; 3]) -> bool {
    state.entities.slots().any(|slot| {
        matches!(
            state.entities.row(slot).special_mob.as_deref(),
            Some(crate::special_mobs::Retained::Warden(_))
        ) && (0..3).all(|i| (state.entities.pos(slot)[i] - pos[i]).abs() <= 48.0)
    })
}
pub(crate) fn shriek(state: &mut TickState, pos: BlockPos, player: EntityId) -> bool {
    if state.players.get(&player).is_none_or(|p| p.spectator()) || block(state, pos).is_none() {
        return false;
    }
    changed(state, pos);
    if state.shriekers.entries[&key(pos)].shriek_until > state.tick {
        return false;
    }
    let centre = vibration::centre(pos);
    let respond = property(state, pos, "can_summon")
        && state.difficulty != crate::survival::Difficulty::Peaceful
        && crate::command_rules::boolean(state, "spawn_wardens");
    let mut level = 0;
    if respond {
        if nearby_warden(state, centre) {
            return false;
        }
        let mut nearby: Vec<_> = state
            .players
            .iter()
            .filter(|(_, p)| !p.spectator())
            .filter_map(|(id, _)| {
                state
                    .entities
                    .slot_of(*id)
                    .filter(|s| {
                        crate::special_mobs::distance_sq(centre, state.entities.pos(*s)) < 256.0
                    })
                    .map(|_| *id)
            })
            .collect();
        if !nearby.contains(&player) {
            nearby.push(player);
        }
        if nearby
            .iter()
            .any(|id| state.players[id].warden_warning.cooldown_ticks() > 0)
        {
            return false;
        }
        level = nearby
            .iter()
            .map(|id| state.players[id].warden_warning.warning_level())
            .max()
            .unwrap_or(0)
            .saturating_add(1)
            .min(4);
        for id in nearby {
            state
                .players
                .get_mut(&id)
                .expect("nearby player")
                .warden_warning = Warning::warned(level, state.tick + 1);
        }
    }
    let entry = state
        .shriekers
        .entries
        .get_mut(&key(pos))
        .expect("shrieker registered");
    entry.warning_level = level;
    entry.shriek_until = state.tick + 90;
    set_shrieking(state, pos, true);
    persist_listener(state, pos);
    let connections: Vec<_> = state.players.values().map(|p| p.connection).collect();
    for connection in connections {
        state.push_player(
            connection,
            PlayerChange::ProjectileLevelEvent {
                position: pos,
                event: 3007,
                data: 0,
            },
        );
    }
    vibration::emit(state, centre, GameEvent::Shriek, Some(player));
    true
}
fn respond(state: &mut TickState, pos: BlockPos, level: u32) {
    if level == 0
        || !property(state, pos, "can_summon")
        || !crate::command_rules::boolean(state, "spawn_wardens")
        || state.difficulty == crate::survival::Difficulty::Peaceful
    {
        return;
    }
    let centre = vibration::centre(pos);
    crate::warden::darkness(state, centre, 40.0);
    if level < 4 || nearby_warden(state, centre) {
        return;
    }
    let Some(type_id) = state
        .registry
        .entity_types
        .id_of(&mc_data::Identifier::parse("minecraft:warden").expect("warden id"))
    else {
        return;
    };
    // Bounded attempts inside vanilla's 11x13x11 volume; no structure/worldgen dependency.
    for n in 0..20 {
        let dx = (n * 7 % 11) - 5;
        let dz = (n * 3 % 11) - 5;
        for dy in (-6..=6).rev() {
            let at = [
                centre[0] + f64::from(dx),
                f64::from(pos.y + dy),
                centre[2] + f64::from(dz),
            ];
            let ground = state
                .world
                .block_at(at[0].floor() as i32, at[1] as i32 - 1, at[2].floor() as i32)
                .is_some_and(|b| {
                    state.registry.materials.classify(b) == mc_physics::BlockMaterial::Solid
                });
            if !ground || !clear_body(state, at) {
                continue;
            }
            let id = state.spawn_mob(type_id, at, 500.0);
            crate::warden::emerging(state, id);
            state.hostile.spawned.push(id);
            return;
        }
    }
}
fn clear_body(state: &TickState, at: [f64; 3]) -> bool {
    let bounds = [
        (at[0] - 0.45, at[0] + 0.45),
        (at[1], at[1] + 2.9 - 1e-6),
        (at[2] - 0.45, at[2] + 0.45),
    ];
    for x in bounds[0].0.floor() as i32..=bounds[0].1.floor() as i32 {
        for y in bounds[1].0.floor() as i32..=bounds[1].1.floor() as i32 {
            for z in bounds[2].0.floor() as i32..=bounds[2].1.floor() as i32 {
                if state.world.block_at(x, y, z).is_none_or(|b| {
                    state.registry.materials.classify(b) != mc_physics::BlockMaterial::Air
                }) {
                    return false;
                }
            }
        }
    }
    !state.entities.slots().any(|slot| {
        let pos = state.entities.pos(slot);
        let row = state.entities.body(slot);
        row.alive
            && (pos[0] - at[0]).abs() < row.half_width + 0.45
            && (pos[2] - at[2]).abs() < row.half_width + 0.45
            && pos[1] < at[1] + 2.9
            && pos[1] + row.height > at[1]
    })
}
pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, _) in writes {
        if block(state, pos).is_some() {
            changed(state, pos);
            persist_listener(state, pos);
        }
    }
}
pub(crate) fn run(state: &mut TickState) {
    for player in state.players.values_mut() {
        // This phase runs before the simulation clock publishes the next tick.
        player.warden_warning.advance_tick(state.tick + 1);
    }
    hydrate(state);
    let mut positions = std::mem::take(&mut state.shriekers.positions);
    positions.clear();
    positions.extend(state.shriekers.entries.keys().copied());
    for &(x, y, z) in &positions {
        let pos = BlockPos { x, y, z };
        let Some(mut entry) = state.shriekers.entries.get(&key(pos)).copied() else {
            continue;
        };
        if block(state, pos).is_none() {
            state.shriekers.entries.remove(&key(pos));
            vibration::unlisten(state, pos);
            continue;
        }
        if !state
            .world
            .is_active(pos.x.div_euclid(16), pos.z.div_euclid(16))
        {
            state.shriekers.entries.insert(key(pos), entry);
            continue;
        }
        let mut changed_image = false;
        if entry.shriek_until != 0 && state.tick >= entry.shriek_until {
            entry.shriek_until = 0;
            set_shrieking(state, pos, false);
            respond(state, pos, entry.warning_level);
            changed_image = true;
        }
        state.shriekers.entries.insert(key(pos), entry);
        if let Some(v) = vibration::take(state, pos) {
            let shrieked = v.event == GameEvent::TendrilsClicking
                && v.source.is_some_and(|player| shriek(state, pos, player));
            changed_image |= !shrieked;
        }
        // Walking directly onto the shrieker is the other vanilla activation path.
        if !property(state, pos, "shrieking") {
            let player = state
                .players
                .iter()
                .filter(|(_, p)| !p.spectator())
                .filter_map(|(id, _)| {
                    state
                        .entities
                        .slot_of(*id)
                        .filter(|s| {
                            let p = state.entities.pos(*s);
                            p[0].floor() as i32 == pos.x
                                && p[2].floor() as i32 == pos.z
                                && p[1] >= pos.y as f64
                                && p[1] < pos.y as f64 + 1.2
                        })
                        .map(|_| *id)
                })
                .min();
            if let Some(player) = player {
                shriek(state, pos, player);
            }
        }
        if changed_image {
            persist_listener(state, pos);
        }
    }
    state.shriekers.positions = positions;
}
#[cfg(test)]
#[path = "shrieker_tests.rs"]
mod tests;
