//! Native hanging paintings. The journaled variant/direction owns attachment.
use crate::boundary::{BlockPos, ChangeSet, EntityDataValue, EntityId, Hand};
use crate::entities::EntitySlot;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use std::sync::Arc;

#[derive(Clone, Debug)]
pub(crate) struct Variant { pub name: Identifier, width: i32, height: i32 }
pub(crate) fn load(root: &std::path::Path) -> Result<Vec<Variant>, crate::CoreError> {
    let dir = root.join("data/minecraft/painting_variant");
    let fail = |error: std::io::Error| crate::CoreError::Registry(format!("painting variants: {error}"));
    let mut files = std::fs::read_dir(dir).map_err(fail)?.map(|entry| entry.map(|e| e.path())).collect::<Result<Vec<_>,_>>().map_err(fail)?;
    files.retain(|path| path.extension().is_some_and(|ext| ext == "json"));
    files.sort();
    files.into_iter().map(|path| {
        let data: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).map_err(fail)?).map_err(|e| crate::CoreError::Registry(e.to_string()))?;
        let name = Identifier::parse(format!("minecraft:{}",path.file_stem().and_then(|s| s.to_str()).unwrap_or(""))).map_err(|e| crate::CoreError::Registry(e.to_string()))?;
        let dimension = |key| data.get(key).and_then(|v| v.as_i64()).filter(|&v| (1..=16).contains(&v)).map(|v| v as i32).ok_or_else(|| crate::CoreError::Registry(format!("{name}: invalid painting {key}")));
        Ok(Variant { width: dimension("width")?, height: dimension("height")?, name })
    }).collect()
}
fn type_id(state: &TickState) -> u32 { state.registry.entity_type_id("minecraft:painting").expect("painting type") }
fn is_painting(state: &TickState, slot: EntitySlot) -> bool { matches!(state.entities.body(slot).kind, crate::entities::EntityKind::Mob { type_id: id } if id == type_id(state)) }
fn variant(state: &TickState, slot: EntitySlot) -> Option<usize> {
    state.entities.row(slot).appearance.extra.iter().find_map(|(index,value)| match (index,value) { (9,EntityDataValue::PaintingVariant(id)) => Some(*id as usize), _ => None })
}
fn anchor(state: &TickState, slot: EntitySlot) -> BlockPos {
    let [x,y,z] = state.entities.pos(slot).map(|v| v.floor() as i32);
    BlockPos { x,y,z }
}
fn cell(anchor: BlockPos, face: u8, x: i32, y: i32) -> BlockPos {
    let left = match face { 2 => [-1,0], 3 => [1,0], 4 => [0,1], _ => [0,-1] };
    BlockPos { x: anchor.x + x*left[0], y: anchor.y+y, z: anchor.z+x*left[1] }
}
fn cells(anchor: BlockPos, face: u8, variant: &Variant) -> impl Iterator<Item=BlockPos> + use<> {
    let (width,height) = (variant.width,variant.height);
    (0..width).flat_map(move |x| (0..height).map(move |y| cell(anchor,face,x-(width-1)/2,y-(height-1)/2)))
}
fn fits(state: &TickState, pos: BlockPos, face: u8, variant: &Variant, except: Option<EntityId>) -> bool {
    let step = crate::placement::Face::from_code(face).offset();
    for cell in cells(pos,face,variant) {
        let Some(front) = state.world.block_at(cell.x,cell.y,cell.z) else { return false };
        if !crate::tick::replaceable(state,front) { return false }
        let Some(back) = state.world.block_at(cell.x-step[0],cell.y,cell.z-step[2]) else { return false };
        if !state.registry.materials.classify(back).is_solid() { return false }
        for slot in state.entities.mob_slots(type_id(state)) {
            if Some(state.entities.body(slot).id) == except { continue }
            let Some(other) = self::variant(state,slot).and_then(|id| state.registry.paintings.get(id)) else { continue };
            if cells(anchor(state,slot),state.entities.row(slot).appearance.spawn_data as u8,other).any(|other| other == cell) { return false }
        }
    }
    true
}
pub(crate) fn use_on(state: &mut TickState, actor: EntityId, hand: Hand, clicked: BlockPos, face: u8) -> bool {
    let Some(player) = state.players.get(&actor) else { return false };
    let held = &player.inventory.slots[crate::tick::held_slot_index(player,hand)];
    if held.is_empty() || !state.registry.items.name_of(held.item_id).is_some_and(|id| id.as_str() == "minecraft:painting") { return false }
    if player.spectator() || player.vitals.is_dead() || !(2..=5).contains(&face) || crate::tick::held_debit_below_floor(state,actor,hand,1) { return true }
    let step = crate::placement::Face::from_code(face).offset();
    let pos = BlockPos { x: clicked.x+step[0],y:clicked.y,z:clicked.z+step[2] };
    let mut chosen = None;
    let mut area = 0;
    let mut ties = 0;
    let mut rng = mc_data::loot::chest_26_1_2::ChestRng::new(state.tick ^ actor.0 as u64);
    for (id,variant) in state.registry.paintings.iter().enumerate() {
        let candidate = variant.width*variant.height;
        if candidate < area || !state.registry.tags.contains_raw_id("minecraft:painting_variant","minecraft:placeable",id as i32) || !fits(state,pos,face,variant,None) { continue }
        if candidate > area { area = candidate; ties = 0; }
        ties += 1;
        if rng.next_bounded(ties) == 0 { chosen = Some(id); }
    }
    let Some(variant) = chosen else { return true };
    let id = state.spawn_mob(type_id(state),[f64::from(pos.x),f64::from(pos.y),f64::from(pos.z)],1.0);
    let slot = state.entities.slot_of(id).expect("painting spawned");
    let row = state.entities.row_mut(slot);
    row.persistent = true;
    let appearance = Arc::make_mut(&mut row.appearance);
    appearance.spawn_data = i32::from(face);
    appearance.extra = vec![(8,EntityDataValue::Direction(face)),(9,EntityDataValue::PaintingVariant(variant as u32))];
    restore(state,slot);
    state.hostile.spawned.push(id);
    let player = state.players.get_mut(&actor).expect("painting placer");
    let connection = player.connection;
    let effects = player.consume_held(hand,1);
    for change in effects.changes { state.push_player(connection,change); }
    true
}
pub(crate) fn restore(state: &mut TickState, slot: EntitySlot) {
    if !is_painting(state,slot) { return }
    let Some(variant) = variant(state,slot).and_then(|id| state.registry.paintings.get(id)) else { return };
    let (width,height) = (f64::from(variant.width),f64::from(variant.height));
    let yaw = match state.entities.row(slot).appearance.spawn_data { 2 => 180.0, 3 => 0.0, 4 => 90.0, _ => 270.0 };
    state.entities.set_rot(slot,yaw,0.0);
    state.entities.set_half_width(slot,width*0.5);
    let (body,row) = state.entities.parts_mut(slot);
    body.movement = crate::blueprint::MovementClass::Stationary;
    body.height = height;
    row.controls.no_ai = true;
}
fn drop_item(state: &mut TickState, slot: EntitySlot, spawned: &mut Vec<EntityId>) {
    if crate::command_rules::boolean(state,"entity_drops") {
        let item = state.registry.items.id_of(&Identifier::parse("minecraft:painting").unwrap()).expect("painting item");
        spawned.push(crate::tick::spawn_item_entity(state,state.entities.pos(slot),ItemStack::new(item,1)));
    }
}
fn remove(state: &mut TickState, slot: EntitySlot, drop: bool, spawned: &mut Vec<EntityId>) {
    if drop { drop_item(state,slot,spawned); }
    state.hostile.removed.push(state.entities.body(slot).id);
    state.remove_entity(slot);
}
pub(crate) fn attack(state: &mut TickState, actor: EntityId, target: EntityId, changes: &mut ChangeSet) -> bool {
    let Some(slot) = state.entities.slot_of(target).filter(|&slot| is_painting(state,slot)) else { return false };
    let Some(player) = state.players.get(&actor) else { return true };
    if player.spectator() || player.game_mode == crate::commands::GameMode::Adventure { return true }
    remove(state,slot,!player.creative(),&mut changes.entities_spawned);
    true
}
pub(crate) fn death(state: &mut TickState, slot: EntitySlot, spawned: &mut Vec<EntityId>) -> bool {
    if !is_painting(state,slot) { return false }
    state.entities.row_mut(slot).loot.death_handled = true;
    drop_item(state,slot,spawned);
    true
}
pub(crate) fn blocks_changed(state: &mut TickState, writes: &[(BlockPos,u32)], spawned: &mut Vec<EntityId>) {
    if writes.is_empty() { return }
    let invalid: Vec<_> = state.entities.mob_slots(type_id(state)).filter(|&slot| {
        let Some(variant) = variant(state,slot).and_then(|id| state.registry.paintings.get(id)) else { return false };
        let pos = anchor(state,slot);
        let face = state.entities.row(slot).appearance.spawn_data as u8;
        let step = crate::placement::Face::from_code(face).offset();
        writes.iter().any(|(changed,_)| cells(pos,face,variant).any(|cell| cell == *changed || BlockPos { x:cell.x-step[0],y:cell.y,z:cell.z-step[2] } == *changed)) && !fits(state,pos,face,variant,Some(state.entities.body(slot).id))
    }).map(|slot| state.entities.body(slot).id).collect();
    for id in invalid { if let Some(slot) = state.entities.slot_of(id) { remove(state,slot,true,spawned); } }
}

pub(crate) fn capture(record: &mut mc_data::entity_nbt::VanillaEntity, appearance: &crate::boundary::EntityAppearance, registry: &crate::registry::CoreRegistry) {
    if record.id != "minecraft:painting" { return }
    record.extra_fields.retain(|(name,_)| !matches!(name.as_str(),"facing" | "variant"));
    let face = match appearance.spawn_data { 3 => 0, 4 => 1, 2 => 2, _ => 3 };
    record.extra_fields.push(("facing".into(),mc_nbt::Tag::Byte(face)));
    for (index,value) in &appearance.extra {
        if let (9,EntityDataValue::PaintingVariant(id)) = (index,value) {
            record.extra_fields.push(("variant".into(),mc_nbt::Tag::String(registry.paintings[*id as usize].name.to_string())));
        }
    }
}
pub(crate) fn native_appearance(record: &mc_data::entity_nbt::VanillaEntity, image: &mut crate::journal::EntityAppearanceImage, registry: &crate::registry::CoreRegistry) -> Result<(),crate::CoreError> {
    if record.id != "minecraft:painting" { return Ok(()) }
    use mc_data::item_nbt::field;
    use mc_nbt::Tag;
    let invalid = || crate::CoreError::Storage("invalid painting facing/variant".into());
    let face = match field(&record.extra_fields,"facing") { Some(Tag::Byte(value)) if (0..4).contains(value) => [3,4,2,5][*value as usize], _ => return Err(invalid()) };
    let Some(Tag::String(name)) = field(&record.extra_fields,"variant") else { return Err(invalid()) };
    let variant = registry.paintings.iter().position(|v| v.name.as_str() == name).ok_or_else(invalid)?;
    image.spawn_data = face;
    image.extra.push((8,crate::journal::EntityDataValueImage::Direction(face as u8)));
    image.extra.push((9,crate::journal::EntityDataValueImage::PaintingVariant(variant as u32)));
    Ok(())
}

pub(crate) fn validate_image(record: &crate::journal::ReplayEntity, registry: &crate::registry::CoreRegistry) -> Result<(), crate::CoreError> {
    use crate::journal::EntityDataValueImage;
    let invalid = || crate::CoreError::Storage("invalid painting metadata image".into());
    if !(2..=5).contains(&record.appearance.spawn_data) { return Err(invalid()) }
    let mut direction = false;
    let mut variant = false;
    for (index, value) in &record.appearance.extra {
        match (*index, value) {
            (8, EntityDataValueImage::Direction(face)) if !direction && i32::from(*face) == record.appearance.spawn_data => direction = true,
            (9, EntityDataValueImage::PaintingVariant(id)) if !variant && (*id as usize) < registry.paintings.len() => variant = true,
            (8 | 9, _) => return Err(invalid()),
            _ => {}
        }
    }
    if !direction || !variant { return Err(invalid()) }
    Ok(())
}
