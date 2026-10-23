//! First-open loot resolves on the existing locate worker; only tick completion
//! can publish maps and inventory. No map ids are allocated by pending requests.
use std::sync::Arc;
use mc_data::{Identifier, ItemStack};
use mc_data::loot::chest_26_1_2::{ChestLootContext, ChestRng, ExplorationMap};
use mc_world::{BlockPos, ChunkPos};
use crate::boundary::{ConnectionId, PlayerInput};
use crate::dimensions::Dimension;
use crate::tick::{BlockEntityState, TickState};

type Key = (Dimension, [i32; 3]);
#[derive(Debug)]
pub(crate) struct Pending {
    revision: u64,
    table: Identifier,
    seed: i64,
    bytes: Vec<u8>,
    specs: Vec<ExplorationMap>,
    hits: Vec<Option<BlockPos>>,
    openers: Vec<(ConnectionId, PlayerInput)>,
}
fn position(key: Key) -> BlockPos {
    let [x,y,z] = key.1; BlockPos { x,y,z }
}
fn source(state: &TickState, key: Key) -> Option<(Identifier, i64, Option<&[u8]>)> {
    if state.dimension != key.0 { return None; }
    let pos = position(key);
    let chunk = state.world.chunk(ChunkPos { x: pos.x.div_euclid(16), z: pos.z.div_euclid(16) })?;
    let chest = chunk.chests.get(&pos)?;
    if state.block_entities.contains_key(&(pos.x,pos.y,pos.z)) { return None; }
    Some((chest.loot_table.clone()?, chest.loot_seed, chunk.block_entities.get(&pos).map(Vec::as_slice)))
}
fn current(state: &TickState, key: Key, pending: &Pending) -> bool {
    let pos = position(key);
    state.world.chunk_revision(pos.x.div_euclid(16),pos.z.div_euclid(16)) == pending.revision
        && source(state,key).is_some_and(|(table, seed, bytes)| table == pending.table && seed == pending.seed
            && bytes.is_none_or(|bytes| bytes == pending.bytes))
}
/// Returns true when opening must wait (or the saved table is unsupported).
pub(crate) fn before_open(state: &mut TickState, connection: ConnectionId, input: &PlayerInput,
    cells: &[(i32,i32,i32)]) -> bool {
    let mut waiting = false;
    for &(x,y,z) in cells {
        let key = (state.dimension,[x,y,z]);
        if let Some(pending) = state.maps.pending_loot.get_mut(&key) {
            if !pending.openers.iter().any(|(id,_)| *id == connection) {
                pending.openers.push((connection,input.clone()));
            }
            waiting = true;
            continue;
        }
        let Some((table,seed,bytes)) = source(state,key) else { continue; };
        let Some(compiled) = state.registry.chest_loot.get(&table) else { waiting = true; continue; };
        let bytes = match bytes {
            Some(bytes) => bytes.to_vec(),
            None => {
                let tag = mc_nbt::Tag::Compound(vec![
                    ("id".into(),mc_nbt::Tag::String("minecraft:chest".into())),
                    ("x".into(),mc_nbt::Tag::Int(x)),("y".into(),mc_nbt::Tag::Int(y)),("z".into(),mc_nbt::Tag::Int(z)),
                    ("LootTable".into(),mc_nbt::Tag::String(table.to_string())),
                    ("LootTableSeed".into(),mc_nbt::Tag::Long(seed))]);
                let mut bytes = Vec::new();
                mc_nbt::write_network(&mut bytes,&tag).expect("generated chest metadata");
                bytes
            }
        };
        let mut pending = Pending {
            revision: state.world.chunk_revision(x.div_euclid(16),z.div_euclid(16)),
            table, seed, bytes, specs: compiled.exploration_maps().to_vec(),
            hits: Vec::new(), openers: vec![(connection,input.clone())],
        };
        if pending.specs.is_empty() {
            pending.openers.clear();
            if resolve(state,key,pending).is_err() { waiting = true; }
        } else {
            state.maps.pending_loot.insert(key,pending);
            if !submit(state,key) { state.maps.pending_loot.remove(&key); }
            waiting = true;
        }
    }
    waiting
}
fn submit(state: &mut TickState, key: Key) -> bool {
    let pending = &state.maps.pending_loot[&key];
    let spec = &pending.specs[pending.hits.len()];
    let referenced = state.maps.records.values().filter(|map| map.dimension == key.0)
        .filter_map(|map| map.destination.map(|[x,y,z]| BlockPos{x,y,z})).collect();
    crate::command_locate::submit_structure_lookup(state, crate::command_locate::StructureLookupRequest {
        connection: pending.openers[0].0, origin: (key.1[0],key.1[1],key.1[2]),
        destination: spec.destination.clone(), search_radius: spec.search_radius,
        skip_existing_chunks: spec.skip_existing_chunks, referenced,
        reply: Box::new(move |state, hit| completed(state,key,hit)),
    })
}
fn completed(state: &mut TickState, key: Key, hit: Option<BlockPos>) {
    let Some(mut pending) = state.maps.pending_loot.remove(&key) else { return; };
    if !current(state,key,&pending) { return; }
    if pending.specs[pending.hits.len()].skip_existing_chunks
        && hit.is_some_and(|pos| state.maps.records.values().any(|map|
            map.dimension == key.0 && map.destination == Some([pos.x,pos.y,pos.z]))) {
        state.maps.pending_loot.insert(key,pending);
        if !submit(state,key) {state.maps.pending_loot.remove(&key);}
        return;
    }
    pending.hits.push(hit);
    if pending.hits.len() < pending.specs.len() {
        state.maps.pending_loot.insert(key,pending);
        if !submit(state,key) { state.maps.pending_loot.remove(&key); }
        return;
    }
    let _ = resolve(state,key,pending);
}
fn resolve(state: &mut TickState, key: Key, pending: Pending) -> Result<(), String> {
    if !current(state,key,&pending) { return Err("stale unopened container".into()); }
    let registry = Arc::clone(&state.registry);
    let table = registry.chest_loot.get(&pending.table).ok_or("missing loot table")?;
    let mut maps = Vec::new();
    let base = state.maps.next_id;
    let mut resolver = |spec: &ExplorationMap| -> Result<Option<ItemStack>, String> {
        let index = pending.specs.iter().position(|known| known == spec).ok_or("unplanned exploration map")?;
        let Some(pos) = pending.hits[index] else { return Ok(None); };
        let map = super::exploration([pos.x,pos.y,pos.z],key.0,spec.decoration.as_str(),spec.zoom)?;
        let id = i32::try_from(base + maps.len() as i64).map_err(|_| "map ids exhausted")?;
        let mut stack = super::filled_map(state,id)?;
        super::attach_decorations(&mut stack,&map);
        maps.push(map);
        Ok(Some(stack))
    };
    let mut rng = ChestRng::new(pending.seed as u64);
    let stacks = table.roll_with_context(&mut rng,&registry.items,&mut ChestLootContext {
        luck: 0.0, exploration_map: Some(&mut resolver),
    }).map_err(|e| e.to_string())?;
    let mut slots = vec![ItemStack::EMPTY;27];
    let mut available: Vec<usize> = (0..27).collect();
    for stack in stacks {
        if available.is_empty() { break; }
        let index = rng.next_bounded(available.len() as u64) as usize;
        slots[available.swap_remove(index)] = stack;
    }
    let mut tag = mc_nbt::read_network(&mut pending.bytes.as_slice()).map_err(|e| e.to_string())?;
    let mc_nbt::Tag::Compound(fields) = &mut tag else { return Err("container compound expected".into()); };
    fields.retain(|(key,_)| key != "LootTable" && key != "LootTableSeed");
    let mut bytes = Vec::new();
    mc_nbt::write_network(&mut bytes,&tag).map_err(|e| e.to_string())?;
    // All fallible planning has finished. These postimages share this tick WAL.
    for map in maps { state.maps.allocate(map).expect("preflighted map allocation"); }
    let pos = position(key);
    state.block_entities.insert((pos.x,pos.y,pos.z), BlockEntityState { slots, furnace: None });
    state.world.set_opaque_block_entity(pos,Some(bytes.clone()));
    state.opaque_changes.push((crate::boundary::BlockPos { x:pos.x,y:pos.y,z:pos.z },Some(bytes)));
    for (connection,input) in pending.openers {
        if state.connections.entity_of(connection).is_some() { state.deferred_uses.push(input); }
    }
    Ok(())
}
