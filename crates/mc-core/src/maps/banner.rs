use super::{Banner, map_id};
use crate::boundary::{BlockPos, EntityId, Hand};
use crate::tick::TickState;
use std::sync::Arc;

pub(crate) fn use_on(state: &mut TickState, actor: EntityId, hand: Hand, pos: BlockPos) -> bool {
    let Some(player) = state.players.get(&actor) else { return false; };
    let held = &player.inventory.slots[crate::tick::held_slot_index(player,hand)];
    let Some(id) = map_id(held) else { return false; };
    let Some(raw) = state.world.block_at(pos.x,pos.y,pos.z) else { return false; };
    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(raw)) else { return false; };
    let name = block.block.id.path();
    let Some(color) = name.strip_suffix("_wall_banner").or_else(|| name.strip_suffix("_banner")) else { return false; };
    let Some(map) = state.maps.records.get_mut(&id) else { return true; };
    if player.spectator() || map.locked || map.dimension != state.dimension { return true; }
    let scale = f64::from(1_u32 << map.scale);
    if (f64::from(pos.x-map.center[0])/scale).abs() > 63.0 || (f64::from(pos.z-map.center[1])/scale).abs() > 63.0 { return true; }
    let at = [pos.x,pos.y,pos.z];
    let map = Arc::make_mut(map);
    if let Some(index) = map.banners.iter().position(|banner| banner.pos == at) {
        map.banners.remove(index);
    } else {
        if map.banners.len() >= 256 { return true; }
        let custom_name = state.world.chunk(mc_world::ChunkPos{x:pos.x.div_euclid(16),z:pos.z.div_euclid(16)})
            .and_then(|chunk| chunk.block_entities.get(&mc_world::BlockPos{x:pos.x,y:pos.y,z:pos.z}))
            .and_then(|bytes| mc_nbt::read_network(&mut bytes.as_slice()).ok())
            .and_then(|tag| if let mc_nbt::Tag::Compound(fields) = tag {
                fields.into_iter().find_map(|(key,value)| if key == "CustomName" { if let mc_nbt::Tag::String(name) = value {Some(name)} else {None} } else {None})
            } else {None});
        map.banners.push(Banner {pos:at,color:color.into(),name:custom_name});
    }
    state.maps.dirty.insert(id);
    true
}
