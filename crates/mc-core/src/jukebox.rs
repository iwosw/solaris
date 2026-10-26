//! Native jukebox record custody and song clock; allays query playing positions.
//! Verified JukeboxBlockEntity/JukeboxSongPlayer 26.1.2: RecordItem, optional
//! ticks_since_song_started, level events 1010(song registry id)/1011(0).
use crate::bees::{block, centre, distance, fail, field, property};
use crate::boundary::{BlockPos, ChangeSet, EntityId, Hand, PlayerChange};
use crate::config::CoreError;
use crate::tick::{BlockEntityState, TickState};
use mc_data::item_nbt::{stack_from_fields, stack_to_fields};
use mc_data::{Identifier, ItemStack};
use mc_nbt::Tag;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Jukebox {
    record: ItemStack,
    elapsed: Option<u64>,
}
pub(crate) type Jukeboxes = BTreeMap<(i32, i32, i32), Jukebox>;
fn key(p: BlockPos) -> (i32, i32, i32) {
    (p.x, p.y, p.z)
}
/// Values come directly from data/vanilla/data/minecraft/jukebox_song/*.json.
pub(crate) fn song(
    items: &mc_data::items::ItemRegistry,
    stack: &ItemStack,
) -> Option<(&'static str, u64, u8)> {
    if stack.is_empty() {
        return None;
    }
    let name = items
        .name_of(stack.item_id)?
        .path()
        .strip_prefix("music_disc_")?;
    Some(match name {
        "11" => ("11", 1440, 11),
        "13" => ("13", 3580, 1),
        "5" => ("5", 3580, 15),
        "blocks" => ("blocks", 6920, 3),
        "cat" => ("cat", 3720, 2),
        "chirp" => ("chirp", 3720, 4),
        "creator" => ("creator", 3540, 12),
        "creator_music_box" => ("creator_music_box", 1480, 11),
        "far" => ("far", 3500, 5),
        "lava_chicken" => ("lava_chicken", 2700, 9),
        "mall" => ("mall", 3960, 6),
        "mellohi" => ("mellohi", 1940, 7),
        "otherside" => ("otherside", 3920, 14),
        "pigstep" => ("pigstep", 3000, 13),
        "precipice" => ("precipice", 6000, 13),
        "relic" => ("relic", 4380, 14),
        "stal" => ("stal", 3020, 8),
        "strad" => ("strad", 3780, 9),
        "tears" => ("tears", 3520, 10),
        "wait" => ("wait", 4780, 12),
        "ward" => ("ward", 5040, 10),
        _ => return None,
    })
}
pub(crate) fn accepts(state: &TickState, stack: &ItemStack) -> bool {
    song(&state.registry.items, stack).is_some()
}
impl Jukebox {
    pub(crate) fn decode(
        mut bytes: &[u8],
        items: &mc_data::items::ItemRegistry,
    ) -> Result<Self, CoreError> {
        let tag = mc_nbt::read_network(&mut bytes).map_err(|e| fail("Jukebox", &e.to_string()))?;
        if !bytes.is_empty() {
            return Err(fail("Jukebox", "trailing NBT bytes"));
        }
        let Tag::Compound(fields) = tag else {
            return Err(fail("Jukebox", "expected compound"));
        };
        let record = match field(&fields, "RecordItem")? {
            None => ItemStack::EMPTY,
            Some(Tag::Compound(fields)) => stack_from_fields(items, fields)
                .map_err(|e| fail("Jukebox.RecordItem", &e.to_string()))?
                .ok_or_else(|| fail("Jukebox.RecordItem", "missing item id"))?,
            Some(_) => return Err(fail("Jukebox.RecordItem", "expected item compound")),
        };
        if !record.is_empty() && (record.count != 1 || song(items, &record).is_none()) {
            return Err(fail("Jukebox.RecordItem", "expected one playable record"));
        }
        let elapsed = match field(&fields, "ticks_since_song_started")? {
            None => None,
            Some(Tag::Long(v)) if *v >= 0 => Some(*v as u64),
            Some(_) => {
                return Err(fail(
                    "Jukebox.ticks_since_song_started",
                    "expected nonnegative long",
                ));
            }
        };
        let elapsed = elapsed
            .filter(|ticks| song(items, &record).is_some_and(|(_, length, _)| *ticks < length));
        Ok(Self { record, elapsed })
    }
    fn encode(&self, pos: BlockPos, items: &mc_data::items::ItemRegistry) -> Vec<u8> {
        let mut fields = vec![
            ("id".into(), Tag::String("minecraft:jukebox".into())),
            ("x".into(), Tag::Int(pos.x)),
            ("y".into(), Tag::Int(pos.y)),
            ("z".into(), Tag::Int(pos.z)),
        ];
        fields.extend(self.data_fields(items));
        let mut bytes = Vec::new();
        mc_nbt::write_network(&mut bytes, &Tag::Compound(fields)).expect("jukebox NBT");
        bytes
    }
    pub(crate) fn data_fields(&self, items: &mc_data::items::ItemRegistry) -> Vec<(String, Tag)> {
        let mut fields = Vec::new();
        if !self.record.is_empty() {
            fields.push((
                "RecordItem".into(),
                Tag::Compound(stack_to_fields(items, &self.record).expect("registered record")),
            ));
        }
        if let Some(ticks) = self.elapsed {
            fields.push(("ticks_since_song_started".into(), Tag::Long(ticks as i64)));
        }
        fields
    }
}
pub(crate) fn decode_chunk(
    boxes: &Jukeboxes,
    registry: &crate::registry::CoreRegistry,
    chunk: &mc_world::Chunk,
) -> Result<Vec<((i32, i32, i32), Jukebox)>, CoreError> {
    let mut decoded = Vec::new();
    for (pos, bytes) in &chunk.block_entities {
        if chunk
            .get_block(
                pos.x.rem_euclid(16) as u8,
                pos.y,
                pos.z.rem_euclid(16) as u8,
            )
            .and_then(|id| registry.blocks.by_id(id))
            .is_some_and(|b| b.block.id.path() == "jukebox")
            && !boxes.contains_key(&key(*pos))
        {
            decoded.push((key(*pos), Jukebox::decode(bytes, &registry.items)?));
        }
    }
    Ok(decoded)
}
pub(crate) fn apply_chunk(
    boxes: &mut Jukeboxes,
    inventories: &mut crate::redstone::custody::BlockEntities,
    decoded: Vec<((i32, i32, i32), Jukebox)>,
) {
    for (key, jukebox) in decoded {
        inventories.hydrate(key, || BlockEntityState {
            slots: vec![jukebox.record.clone()],
            furnace: None,
        });
        boxes.insert(key, jukebox);
    }
}
/// Persisted jukeboxes enter only through chunk hydration; a missing entry
/// here is a freshly placed block.
pub(crate) fn register(state: &mut TickState, pos: BlockPos) {
    if !state.jukeboxes.contains_key(&key(pos)) {
        let jukebox = Jukebox::default();
        state
            .block_entities
            .entry(key(pos))
            .or_insert_with(|| BlockEntityState {
                slots: vec![jukebox.record.clone()],
                furnace: None,
            });
        state.jukeboxes.insert(key(pos), jukebox);
    }
}
fn event(state: &mut TickState, pos: BlockPos, event: i32, data: i32) {
    let viewers: Vec<_> = state
        .players
        .iter()
        .filter_map(|(id, player)| {
            state
                .entities
                .slot_of(*id)
                .filter(|slot| distance(state.entities.pos(*slot), centre(pos)) <= 64.0 * 64.0)
                .map(|_| player.connection)
        })
        .collect();
    for connection in viewers {
        state.push_player(
            connection,
            PlayerChange::ProjectileLevelEvent {
                position: pos,
                event,
                data,
            },
        );
    }
}
fn persist(state: &mut TickState, pos: BlockPos, value: &Jukebox) {
    let bytes = value.encode(pos, &state.registry.items);
    state
        .world
        .set_opaque_block_entity(pos, Some(bytes.clone()));
    state.opaque_changes.push((pos, Some(bytes)));
}
fn changed(
    state: &mut TickState,
    pos: BlockPos,
    value: &mut Jukebox,
    record: ItemStack,
    writes: &mut Vec<(BlockPos, u32)>,
) {
    if value.elapsed.is_some() {
        event(state, pos, 1011, 0);
    }
    value.record = record;
    value.elapsed = None;
    if let Some((name, _, _)) = song(&state.registry.items, &value.record) {
        let registry_id = mc_data::required_registry_entry_id(
            "jukebox_song",
            &Identifier::parse(format!("minecraft:{name}")).unwrap(),
        )
        .expect("embedded song registry");
        value.elapsed = Some(0);
        event(state, pos, 1010, registry_id as i32);
    }
    if let Some(next) = property(
        state,
        pos,
        "has_record",
        if value.record.is_empty() {
            "false"
        } else {
            "true"
        },
    ) && state.world.block_at(pos.x, pos.y, pos.z) != Some(next)
    {
        state.world.set_block(pos.x, pos.y, pos.z, next);
        writes.push((pos, next));
    }
    for (dx, dy, dz) in [
        (0, 0, 0),
        (1, 0, 0),
        (-1, 0, 0),
        (0, 1, 0),
        (0, -1, 0),
        (0, 0, 1),
        (0, 0, -1),
    ] {
        crate::redstone::schedule_cell(
            state,
            BlockPos {
                x: pos.x + dx,
                y: pos.y + dy,
                z: pos.z + dz,
            },
            1,
            0,
        );
    }
    persist(state, pos, value);
}
pub(crate) fn use_on(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    pos: BlockPos,
    changes: &mut ChangeSet,
) -> bool {
    if block(state, pos).is_none_or(|b| b.block.id.path() != "jukebox") {
        return false;
    }
    let Some(player) = state.players.get(&actor) else {
        return true;
    };
    if player.spectator() {
        return true;
    }
    let connection = player.connection;
    let held = player.inventory.slots[crate::tick::held_slot_index(player, hand)].clone();
    register(state, pos);
    let mut boxes = std::mem::take(&mut state.jukeboxes);
    let value = boxes.get_mut(&key(pos)).unwrap();
    let mut writes = Vec::new();
    if !value.record.is_empty() {
        let item = std::mem::take(&mut value.record);
        changed(state, pos, value, ItemStack::EMPTY, &mut writes);
        state.block_entities.get_mut(&key(pos)).unwrap().slots[0] = ItemStack::EMPTY;
        changes
            .entities_spawned
            .push(crate::tick::spawn_item_entity(state, centre(pos), item));
    } else if accepts(state, &held) {
        if crate::tick::held_debit_below_floor(state, actor, hand, 1) {
            state.jukeboxes = boxes;
            return true;
        }
        let mut record = held.clone();
        record.count = 1;
        state.block_entities.get_mut(&key(pos)).unwrap().slots[0] = record.clone();
        changed(state, pos, value, record, &mut writes);
        let effects = state.players.get_mut(&actor).unwrap().consume_held(hand, 1);
        for change in effects.changes {
            state.push_player(connection, change);
        }
        crate::advancements::accepted_block(
            state,
            actor,
            "minecraft:item_used_on_block",
            pos,
            None,
            Some(held),
        );
    }
    state.merge_block_writes.extend(writes);
    state.jukeboxes = boxes;
    true
}
pub(crate) fn run(
    state: &mut TickState,
    writes: &mut Vec<(BlockPos, u32)>,
    drops: &mut Vec<EntityId>,
) {
    let mut boxes = std::mem::take(&mut state.jukeboxes);
    boxes.retain(|&(x, y, z), value| {
        let pos = BlockPos { x, y, z };
        if !state
            .world
            .is_block_active(x.div_euclid(16), z.div_euclid(16))
        {
            return true;
        }
        if block(state, pos).is_none_or(|b| b.block.id.path() != "jukebox") {
            if value.elapsed.is_some() {
                event(state, pos, 1011, 0);
            }
            if let Some(mut record) = state.block_entities.remove(&key(pos)) {
                for stack in &mut record.slots {
                    if !stack.is_empty() {
                        drops.push(crate::tick::spawn_item_entity(
                            state,
                            centre(pos),
                            std::mem::take(stack),
                        ));
                    }
                }
            }
            state.world.set_opaque_block_entity(pos, None);
            state.opaque_changes.push((pos, None));
            return false;
        }
        let record = state
            .block_entities
            .get(&key(pos))
            .and_then(|r| r.slots.first());
        if record.map_or_else(
            || value.record != ItemStack::EMPTY,
            |record| record != &value.record,
        ) {
            let record = record.cloned().unwrap_or_default();
            changed(state, pos, value, record, writes);
        }
        if let Some(elapsed) = &mut value.elapsed {
            let (_, length, _) =
                song(&state.registry.items, &value.record).expect("validated song");
            if *elapsed >= length {
                value.elapsed = None;
                event(state, pos, 1011, 0);
                crate::redstone::wake(state, pos);
                persist(state, pos, value);
            } else {
                *elapsed += 1;
            }
        }
        true
    });
    state.jukeboxes = boxes;
}
pub(crate) fn playing(state: &TickState, pos: BlockPos) -> bool {
    state
        .jukeboxes
        .get(&key(pos))
        .is_some_and(|v| v.elapsed.is_some())
}
pub(crate) fn nearby_playing(state: &TickState, pos: [f64; 3]) -> bool {
    state.jukeboxes.iter().any(|(&(x, y, z), v)| {
        v.elapsed.is_some() && distance(pos, centre(BlockPos { x, y, z })) <= 100.0
    })
}
pub(crate) fn comparator(state: &TickState, pos: BlockPos) -> Option<u8> {
    if block(state, pos).is_none_or(|b| b.block.id.path() != "jukebox") {
        return None;
    }
    let record = state
        .block_entities
        .get(&key(pos))
        .and_then(|v| v.slots.first())?;
    Some(song(&state.registry.items, record).map_or(0, |(_, _, signal)| signal))
}
pub(crate) fn checkpoint(state: &mut TickState) {
    for (&(x, y, z), value) in &state.jukeboxes {
        let pos = BlockPos { x, y, z };
        state
            .world
            .set_opaque_block_entity(pos, Some(value.encode(pos, &state.registry.items)));
    }
}
#[cfg(test)]
#[path = "jukebox_tests.rs"]
mod tests;
