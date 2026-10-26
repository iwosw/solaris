//! Ordinary monster spawners. Chunk-indexed activation and event-only delay WAL.
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntityKind;
use crate::tick::TickState;
use mc_data::item_nbt::field;
use mc_data::trial_nbt::{self as nbt, Fields};
use mc_nbt::Tag;
use mc_world::ChunkPos;
use std::collections::BTreeMap;

type Key = (i32, i32, i32);
#[derive(Debug, Default)]
pub(crate) struct Spawners {
    chunks: BTreeMap<(i32, i32), BTreeMap<Key, Spawner>>,
    active: Vec<Key>,
    scratch: Vec<Key>,
}
#[derive(Debug)]
pub(crate) struct Spawner {
    fields: Fields,
    type_id: u32,
    delay: u64,
    deadline: Option<u64>,
    min_delay: u64,
    max_delay: u64,
    count: usize,
    nearby: usize,
    player_range: i32,
    range: i32,
}
fn short(fields: &Fields, key: &str, default: i16, min: i16, max: i16) -> Result<i16, String> {
    let value = match field(fields, key) {
        None => default,
        Some(Tag::Short(value)) => *value,
        _ => return Err(format!("{key}: expected Short")),
    };
    if !(min..=max).contains(&value) {
        return Err(format!("{key}: outside {min}..={max}"));
    }
    Ok(value)
}
impl Spawner {
    pub(crate) fn decode(
        bytes: &[u8],
        registry: &crate::registry::CoreRegistry,
    ) -> Result<Self, String> {
        let fields = nbt::decode(bytes)?;
        let spawn = nbt::compound(&fields, "SpawnData")?.ok_or("SpawnData missing")?;
        let entity = nbt::compound(spawn, "entity")?.ok_or("SpawnData.entity missing")?;
        if entity.iter().any(|(key, _)| key != "id") || spawn.iter().any(|(key, _)| key != "entity")
        {
            return Err("custom spawner entity/rules are unsupported".into());
        }
        if let Some(value) = field(&fields, "SpawnPotentials")
            && !matches!(value, Tag::List(list) if list.elements.is_empty())
        {
            return Err("weighted SpawnPotentials are unsupported".into());
        }
        let Some(Tag::String(name)) = field(entity, "id") else {
            return Err("SpawnData.entity.id missing".into());
        };
        let name = mc_data::Identifier::parse(name).map_err(|error| error.to_string())?;
        let type_id = registry
            .entity_types
            .id_of(&name)
            .ok_or("unknown spawner entity")?;
        let bp = registry
            .blueprint(type_id)
            .ok_or("spawner entity lacks blueprint")?;
        if bp.category != crate::blueprint::MobCategory::Monster
            || bp.half_width > 2.0
            || bp.height > 6.0
        {
            return Err("unsupported spawner body/category".into());
        }
        let min_delay = short(&fields, "MinSpawnDelay", 200, 1, i16::MAX)? as u64;
        let max_delay = short(&fields, "MaxSpawnDelay", 800, min_delay as i16, i16::MAX)? as u64;
        let deadline = match field(&fields, "solaris:spawn_deadline") {
            None => None,
            Some(Tag::Long(value)) if *value >= 0 => Some(*value as u64),
            _ => return Err("solaris:spawn_deadline: expected nonnegative Long".into()),
        };
        Ok(Self {
            type_id,
            delay: short(&fields, "Delay", 20, -1, i16::MAX)?.max(0) as u64,
            deadline,
            min_delay,
            max_delay,
            count: short(&fields, "SpawnCount", 4, 1, 16)? as usize,
            nearby: short(&fields, "MaxNearbyEntities", 6, 1, 128)? as usize,
            player_range: i32::from(short(&fields, "RequiredPlayerRange", 16, 1, 32)?),
            range: i32::from(short(&fields, "SpawnRange", 4, 1, 16)?),
            fields,
        })
    }
    fn persist(&mut self, state: &mut TickState, key: Key) {
        nbt::set(&mut self.fields, "Delay", Tag::Short(self.delay as i16));
        self.fields
            .retain(|(name, _)| name != "solaris:spawn_deadline");
        if let Some(deadline) = self.deadline {
            self.fields
                .push(("solaris:spawn_deadline".into(), Tag::Long(deadline as i64)));
        }
        let pos = BlockPos {
            x: key.0,
            y: key.1,
            z: key.2,
        };
        let bytes = nbt::encode(
            self.fields.clone(),
            "minecraft:mob_spawner",
            [key.0, key.1, key.2],
        );
        state
            .world
            .set_opaque_block_entity(pos, Some(bytes.clone()));
        state.opaque_changes.push((pos, Some(bytes)));
    }
    fn reset(&mut self, tick: u64, rng: &mut crate::natural_spawn::Rng) {
        self.delay = self.min_delay + rng.below((self.max_delay - self.min_delay).max(1));
        self.deadline = Some(tick + self.delay);
    }
}

pub(crate) type Decoded = BTreeMap<Key, Spawner>;

pub(crate) fn decode_chunk(
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<Decoded, String> {
    let mut decoded = BTreeMap::new();
    for (pos, bytes) in &chunk.block_entities {
        if chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|block| block.block.id.path() == "spawner")
        {
            let entry = Spawner::decode(bytes, registry)
                .map_err(|error| format!("monster spawner {pos:?}: {error}"))?;
            decoded.insert((pos.x, pos.y, pos.z), entry);
        }
    }
    Ok(decoded)
}

pub(crate) fn apply_chunk(spawners: &mut Spawners, chunk: ChunkPos, decoded: Decoded) {
    if decoded.is_empty() {
        return;
    }
    let current = spawners.chunks.entry((chunk.x, chunk.z)).or_default();
    for (key, value) in decoded {
        if let std::collections::btree_map::Entry::Vacant(slot) = current.entry(key) {
            if value.deadline.is_some() {
                // A recovered running clock must also be paused if no observer
                // returns; otherwise a later visit would consume offline delay.
                spawners.active.push(key);
            }
            slot.insert(value);
        }
    }
}

pub(crate) fn forget(spawners: &mut Spawners, pos: BlockPos) {
    let key = (pos.x, pos.y, pos.z);
    if let Some(entries) = spawners
        .chunks
        .get_mut(&(pos.x.div_euclid(16), pos.z.div_euclid(16)))
    {
        entries.remove(&key);
    }
    spawners.active.retain(|entry| *entry != key);
}

pub(crate) fn after_blocks(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for &(pos, _) in writes {
        let cell = (pos.x.div_euclid(16), pos.z.div_euclid(16));
        let key = (pos.x, pos.y, pos.z);
        if crate::trial_spawner::block(state, pos, "spawner").is_none() {
            if let Some(entries) = state.monster_spawners.chunks.get_mut(&cell) {
                entries.remove(&key);
            }
        } else if let Some(bytes) = state
            .world
            .chunk(ChunkPos {
                x: cell.0,
                z: cell.1,
            })
            .and_then(|chunk| chunk.block_entities.get(&pos))
        {
            // Generated/imported NBT is admitted at chunk hydration. A new block
            // without SpawnData is intentionally inert until a valid entity is set.
            let spawner = Spawner::decode(bytes, &state.registry)
                .unwrap_or_else(|error| panic!("monster spawner {pos:?}: {error}"));
            state
                .monster_spawners
                .chunks
                .entry(cell)
                .or_default()
                .entry(key)
                .or_insert(spawner);
        }
    }
}

fn nearby(state: &TickState, key: Key, type_id: u32, range: i32, cap: usize) -> usize {
    let mut count = 0;
    // Include bodies straddling the range boundary. Admission caps the
    // spawned blueprint's half-width at two blocks.
    for x in (key.0 - range - 2).div_euclid(16)..=(key.0 + range + 2).div_euclid(16) {
        for z in (key.2 - range - 2).div_euclid(16)..=(key.2 + range + 2).div_euclid(16) {
            state
                .entities
                .for_each_slot_in_chunk(ChunkPos { x, z }, |slot| {
                    let body = state.entities.body(slot);
                    let at = state.entities.pos(slot);
                    if body.alive
                        && body.kind == (EntityKind::Mob { type_id })
                        && (at[0] - (key.0 as f64 + 0.5)).abs()
                            < f64::from(range) + 0.5 + body.half_width
                        && at[1] + body.height > f64::from(key.1 - range)
                        && at[1] < f64::from(key.1 + range + 1)
                        && (at[2] - (key.2 as f64 + 0.5)).abs()
                            < f64::from(range) + 0.5 + body.half_width
                    {
                        count += 1;
                    }
                });
            if count >= cap {
                return count;
            }
        }
    }
    count
}

fn can_spawn(
    state: &TickState,
    type_id: u32,
    at: [f64; 3],
    rng: &mut crate::natural_spawn::Rng,
) -> bool {
    if state.difficulty == crate::survival::Difficulty::Peaceful {
        return false;
    }
    let bp = state
        .registry
        .blueprint(type_id)
        .expect("admitted blueprint");
    let [x, y, z] = at.map(|v| v.floor() as i32);
    let Some((sky, block)) = state.world.light_at(x, y, z) else {
        return false;
    };
    let light_ok = match bp.name.as_str() {
        "minecraft:blaze" => state.brightness(x, y, z) <= 11,
        "minecraft:silverfish" => true,
        _ if state.dimension == crate::dimensions::Dimension::Nether => block <= 11,
        _ => {
            block == 0
                && u64::from(sky) <= rng.below(32)
                && u64::from(state.brightness(x, y, z)) <= rng.below(8)
        }
    };
    if !light_ok {
        return false;
    }
    for bx in (at[0] - bp.half_width).floor() as i32..=(at[0] + bp.half_width - 1e-7).floor() as i32
    {
        for bz in
            (at[2] - bp.half_width).floor() as i32..=(at[2] + bp.half_width - 1e-7).floor() as i32
        {
            for by in y..=(at[1] + bp.height - 1e-7).floor() as i32 {
                if !state.world.block_at(bx, by, bz).is_some_and(|id| {
                    state.registry.materials.classify(id) == mc_physics::BlockMaterial::Air
                }) {
                    return false;
                }
            }
        }
    }
    let mut overlap = false;
    // Blueprint body limits plus the native maximum living half-width bound.
    for cx in (x - 5).div_euclid(16)..=(x + 5).div_euclid(16) {
        for cz in (z - 5).div_euclid(16)..=(z + 5).div_euclid(16) {
            state
                .entities
                .for_each_slot_in_chunk(ChunkPos { x: cx, z: cz }, |slot| {
                    let body = state.entities.body(slot);
                    if !body.alive
                        || matches!(
                            body.kind,
                            EntityKind::Item { .. } | EntityKind::ExperienceOrb { .. }
                        )
                    {
                        return;
                    }
                    let pos = state.entities.pos(slot);
                    overlap |= (at[0] - pos[0]).abs() < bp.half_width + body.half_width
                        && (at[2] - pos[2]).abs() < bp.half_width + body.half_width
                        && at[1] < pos[1] + body.height
                        && at[1] + bp.height > pos[1];
                });
        }
    }
    !overlap
}

pub(crate) fn run(state: &mut TickState, changes: &mut ChangeSet) {
    if state.monster_spawners.chunks.is_empty() {
        return;
    }
    let mut spawners = std::mem::take(&mut state.monster_spawners);
    spawners.scratch.clear();
    for (&id, player) in &state.players {
        if player.game_mode == crate::commands::GameMode::Spectator || player.vitals.is_dead() {
            continue;
        }
        let Some(slot) = state.entities.slot_of(id) else {
            continue;
        };
        let at = state.entities.pos(slot);
        let [px, _, pz] = at.map(|v| v.floor() as i32);
        // RequiredPlayerRange is bounded to 32 at admission: at most 5x5 cells.
        for x in (px - 32).div_euclid(16)..=(px + 32).div_euclid(16) {
            for z in (pz - 32).div_euclid(16)..=(pz + 32).div_euclid(16) {
                if !state.world.is_block_active(x, z) {
                    continue;
                }
                if let Some(entries) = spawners.chunks.get(&(x, z)) {
                    for (&key, entry) in entries {
                        let distance = (at[0] - (key.0 as f64 + 0.5)).powi(2)
                            + (at[1] - (key.1 as f64 + 0.5)).powi(2)
                            + (at[2] - (key.2 as f64 + 0.5)).powi(2);
                        if distance < f64::from(entry.player_range).powi(2) {
                            spawners.scratch.push(key);
                        }
                    }
                }
            }
        }
    }
    spawners.scratch.sort_unstable();
    spawners.scratch.dedup();
    for key in spawners
        .active
        .iter()
        .filter(|key| spawners.scratch.binary_search(key).is_err())
    {
        if let Some(entry) = spawners
            .chunks
            .get_mut(&(key.0.div_euclid(16), key.2.div_euclid(16)))
            .and_then(|entries| entries.get_mut(key))
            && let Some(deadline) = entry.deadline.take()
        {
            entry.delay = deadline.saturating_sub(state.tick).min(entry.delay);
            entry.persist(state, *key);
        }
    }
    for &key in &spawners.scratch {
        let entries = spawners
            .chunks
            .get_mut(&(key.0.div_euclid(16), key.2.div_euclid(16)))
            .expect("indexed cell");
        let entry = entries.get_mut(&key).expect("indexed spawner");
        if crate::trial_spawner::block(
            state,
            BlockPos {
                x: key.0,
                y: key.1,
                z: key.2,
            },
            "spawner",
        )
        .is_none()
        {
            entries.remove(&key);
            continue;
        }
        if entry.deadline.is_none() {
            entry.deadline = Some(state.tick + entry.delay);
            entry.persist(state, key);
        }
        if state.tick < entry.deadline.expect("active deadline") {
            continue;
        }
        let mut rng = crate::natural_spawn::Rng(
            state.tick
                ^ (key.0 as u64).rotate_left(17)
                ^ (key.1 as u64).rotate_left(31)
                ^ key.2 as u64,
        );
        let mut reset = false;
        for _ in 0..entry.count {
            if nearby(state, key, entry.type_id, entry.range, entry.nearby) >= entry.nearby {
                reset = true;
                break;
            }
            let mut offset = || {
                (rng.below(65536) as f64 - rng.below(65536) as f64) / 65536.0
                    * f64::from(entry.range)
            };
            let dx = offset();
            let dz = offset();
            let at = [
                key.0 as f64 + 0.5 + dx,
                key.1 as f64 + rng.below(3) as f64 - 1.0,
                key.2 as f64 + 0.5 + dz,
            ];
            if can_spawn(state, entry.type_id, at, &mut rng) {
                let health = state
                    .registry
                    .blueprint(entry.type_id)
                    .expect("admitted blueprint")
                    .params
                    .max_health;
                changes
                    .entities_spawned
                    .push(state.spawn_mob(entry.type_id, at, health));
                reset = true;
            }
        }
        if reset {
            entry.reset(state.tick, &mut rng);
            entry.persist(state, key);
        }
    }
    std::mem::swap(&mut spawners.active, &mut spawners.scratch);
    state.monster_spawners = spawners;
}

#[cfg(test)]
#[path = "monster_spawner_tests.rs"]
mod tests;
