//! Ordinary mob spawners. Chunk-indexed activation and event-only delay WAL.
use crate::boundary::{BlockPos, ChangeSet};
use crate::entities::EntityKind;
use crate::tick::TickState;
use mc_data::item_nbt::field;
use mc_data::trial_nbt::{self as nbt, Fields};
use mc_nbt::Tag;
use mc_world::ChunkPos;
use std::collections::BTreeMap;

#[path = "monster_spawner_data.rs"]
mod data;

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
    light_rules: Option<[[u8; 2]; 2]>,
    potentials: Vec<(u64, data::SpawnData)>,
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
        let spawn = data::SpawnData::decode(spawn, registry)?;
        let potentials = data::potentials(&fields, registry)?;
        let min_delay = short(&fields, "MinSpawnDelay", 200, 1, i16::MAX)? as u64;
        let max_delay = short(&fields, "MaxSpawnDelay", 800, min_delay as i16, i16::MAX)? as u64;
        let deadline = match field(&fields, "solaris:spawn_deadline") {
            None => None,
            Some(Tag::Long(value)) if *value >= 0 => Some(*value as u64),
            _ => return Err("solaris:spawn_deadline: expected nonnegative Long".into()),
        };
        Ok(Self {
            type_id: spawn.type_id,
            light_rules: spawn.light,
            potentials,
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
        let total: u64 = self.potentials.iter().map(|(weight, _)| weight).sum();
        if total > 0 {
            let mut pick = rng.below(total);
            for (index, (weight, spawn)) in self.potentials.iter().enumerate() {
                if pick >= *weight {
                    pick -= weight;
                    continue;
                }
                self.type_id = spawn.type_id;
                self.light_rules = spawn.light;
                let Some(Tag::List(list)) = field(&self.fields, "SpawnPotentials") else {
                    unreachable!("admitted potentials");
                };
                let Tag::Compound(potential) = &list.elements[index] else {
                    unreachable!("admitted potential");
                };
                let selected = field(potential, "data")
                    .expect("admitted spawn data")
                    .clone();
                nbt::set(&mut self.fields, "SpawnData", selected);
                break;
            }
        }
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
            // Corrupt entries stay inert too: hydration already rejected them
            // without an executable prefix, and the tick must not panic.
            let Ok(spawner) = Spawner::decode(bytes, &state.registry) else {
                continue;
            };
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

/// Spawn eggs change the current spawn data, retaining the clock and potentials.
/// Returns false when the egg's entity cannot be admitted (no blueprint):
/// the caller keeps the normal adjacent-spawn path instead of configuring.
pub(crate) fn set_entity(state: &mut TickState, pos: BlockPos, type_id: u32) -> bool {
    let cell = (pos.x.div_euclid(16), pos.z.div_euclid(16));
    let key = (pos.x, pos.y, pos.z);
    if state.registry.blueprint(type_id).is_none() {
        return false;
    }
    let existing = state
        .monster_spawners
        .chunks
        .get_mut(&cell)
        .and_then(|entries| entries.remove(&key));
    let mut fields = if let Some(entry) = existing {
        entry.fields
    } else if let Some(bytes) = state
        .world
        .chunk(ChunkPos {
            x: cell.0,
            z: cell.1,
        })
        .and_then(|chunk| chunk.block_entities.get(&pos))
    {
        let Ok(decoded) = nbt::decode(bytes) else {
            return false;
        };
        decoded
    } else {
        // A plain placed spawner has no SpawnData until an egg configures it:
        // start from empty fields so the egg becomes the first configuration.
        Vec::new()
    };
    let spawn = nbt::compound(&fields, "SpawnData")
        .ok()
        .flatten()
        .cloned()
        .unwrap_or_default();
    let entity = nbt::compound(&spawn, "entity")
        .ok()
        .flatten()
        .cloned()
        .unwrap_or_default();
    let name = state
        .registry
        .entity_types
        .name_of(type_id)
        .expect("spawn egg entity")
        .as_str();
    let mut entity = entity;
    nbt::set(&mut entity, "id", Tag::String(name.into()));
    let mut spawn = spawn;
    nbt::set(&mut spawn, "entity", Tag::Compound(entity));
    nbt::set(&mut fields, "SpawnData", Tag::Compound(spawn));
    let bytes = nbt::encode(fields, "minecraft:mob_spawner", [pos.x, pos.y, pos.z]);
    let Ok(mut entry) = Spawner::decode(&bytes, &state.registry) else {
        return false;
    };
    entry.persist(state, key);
    let kind = mc_data::block_entity_kinds::BlockEntityKind::MobSpawner;
    state.world.mark_block_entity_visible(pos);
    let wire = Tag::Compound(entry.fields.clone());
    let viewers: Vec<_> = state
        .players
        .values()
        .map(|player| player.connection)
        .collect();
    for connection in viewers {
        state.push_player(
            connection,
            crate::boundary::PlayerChange::BlockEntityData {
                position: pos,
                block_entity_type: kind.protocol_id(),
                nbt: wire.clone(),
            },
        );
    }
    state
        .monster_spawners
        .chunks
        .entry(cell)
        .or_default()
        .insert(key, entry);
    true
}

fn nearby(state: &TickState, key: Key, type_id: u32, range: i32, cap: usize) -> usize {
    let mut count = 0;
    let width = state
        .registry
        .blueprint(type_id)
        .expect("admitted blueprint")
        .half_width
        .ceil() as i32;
    for x in (key.0 - range - width).div_euclid(16)..=(key.0 + range + width).div_euclid(16) {
        for z in (key.2 - range - width).div_euclid(16)..=(key.2 + range + width).div_euclid(16) {
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
    light_rules: Option<[[u8; 2]; 2]>,
) -> bool {
    let bp = state
        .registry
        .blueprint(type_id)
        .expect("admitted blueprint");
    if state.difficulty == crate::survival::Difficulty::Peaceful
        && bp.category == crate::blueprint::MobCategory::Monster
    {
        return false;
    }
    let [x, y, z] = at.map(|v| v.floor() as i32);
    let Some((sky, block)) = state.world.light_at(x, y, z) else {
        return false;
    };
    let light_ok = if let Some([block_range, sky_range]) = light_rules {
        let sky = if state.dimension.has_weather() {
            sky.saturating_sub(crate::light::sky_darken(
                state.day_time,
                state.rain_level,
                state.thunder_level,
            ))
        } else {
            0
        };
        (block_range[0]..=block_range[1]).contains(&block)
            && (sky_range[0]..=sky_range[1]).contains(&sky)
    } else {
        use crate::blueprint::MobCategory;
        match bp.category {
            MobCategory::Monster => match bp.name.as_str() {
                "minecraft:blaze" => state.brightness(x, y, z) <= 11,
                "minecraft:silverfish" => true,
                _ if state.dimension == crate::dimensions::Dimension::Nether => block <= 11,
                _ => {
                    block == 0
                        && u64::from(sky) <= rng.below(32)
                        && u64::from(state.brightness(x, y, z)) <= rng.below(8)
                }
            },
            MobCategory::Creature if bp.name == "minecraft:turtle" => {
                let sand = state
                    .world
                    .block_at(x, y - 1, z)
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|floor| {
                        floor.block.id.path() == "sand" || floor.block.id.path() == "red_sand"
                    });
                sand && y < state.dimension.sea_level() + 4 && sky.max(block) > 8
            }
            MobCategory::Creature => {
                let support = state.world.block_at(x, y - 1, z);
                let grass = support == Some(state.registry.states.grass_block);
                let tagged = support
                    .and_then(|id| state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                    .is_some_and(|floor| {
                        state
                            .registry
                            .tags
                            .raw_ids("minecraft:block", "minecraft:animals_spawnable_on")
                            .is_some_and(|ids| ids.contains(&floor.block.raw_id))
                    });
                (grass || tagged) && sky.max(block) > 8
            }
            MobCategory::Ambient => state.brightness(x, y, z) <= rng.below(4) as u8,
            _ => true,
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
                    let material = state.registry.materials.classify(id);
                    if matches!(bp.movement, crate::blueprint::MovementClass::Swimmer) {
                        material == mc_physics::BlockMaterial::Water
                    } else {
                        material == mc_physics::BlockMaterial::Air
                    }
                }) {
                    return false;
                }
            }
        }
    }
    let mut overlap = false;
    // The native maximum living half-width is three blocks; include the candidate body.
    let extent = (bp.half_width + 3.0).ceil() as i32;
    for cx in (x - extent).div_euclid(16)..=(x + extent).div_euclid(16) {
        for cz in (z - extent).div_euclid(16)..=(z + extent).div_euclid(16) {
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
            if can_spawn(state, entry.type_id, at, &mut rng, entry.light_rules) {
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
