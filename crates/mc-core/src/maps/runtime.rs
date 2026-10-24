use super::{SavedMap, decoration_id, map_id};
use crate::boundary::{ConnectionId, EntityDataValue, PlayerChange};
use crate::tick::TickState;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    pub kind: i32,
    pub x: i8,
    pub z: i8,
    pub rotation: u8,
    pub name: Option<String>,
}
#[derive(Debug, Clone, PartialEq)]
pub struct MapUpdate {
    pub id: i32,
    pub scale: u8,
    pub locked: bool,
    pub markers: Vec<Marker>,
    pub rectangle: [u8; 4], // x, z, width, height; width=0 means no patch
    pub colors: Vec<u8>,
}

pub(super) fn difference(id: i32, map: &SavedMap, old: Option<&SavedMap>) -> MapUpdate {
    let mut min = [128; 2];
    let mut max = [0; 2];
    for (i, &color) in map.colors.iter().enumerate() {
        if old.is_some_and(|old| old.colors[i] == color) {
            continue;
        }
        let p = [i % 128, i / 128];
        for axis in 0..2 {
            min[axis] = min[axis].min(p[axis]);
            max[axis] = max[axis].max(p[axis]);
        }
    }
    let mut update = MapUpdate {
        id,
        scale: map.scale,
        locked: map.locked,
        markers: Vec::new(),
        rectangle: [0; 4],
        colors: Vec::new(),
    };
    if min[0] != 128 {
        let width = max[0] - min[0] + 1;
        let height = max[1] - min[1] + 1;
        update.rectangle = [min[0] as u8, min[1] as u8, width as u8, height as u8];
        update.colors.reserve(width * height);
        for z in min[1]..=max[1] {
            update
                .colors
                .extend_from_slice(&map.colors[z * 128 + min[0]..=z * 128 + max[0]]);
        }
    }
    for d in map.decorations.values() {
        if let Some(kind) = decoration_id(&d.kind) {
            let [x, z] = map.project(d.x, d.z);
            update.markers.push(Marker {
                kind,
                x,
                z,
                rotation: ((d.rotation.rem_euclid(360.0) / 22.5) as u8) & 15,
                name: d.name.clone(),
            });
        }
    }
    for banner in &map.banners {
        let Some(kind) = decoration_id(&format!("banner_{}", banner.color)) else {
            continue;
        };
        let [x, z] = map.project(f64::from(banner.pos[0]), f64::from(banner.pos[2]));
        update.markers.push(Marker {
            kind,
            x,
            z,
            rotation: 8,
            name: banner.name.clone(),
        });
    }
    update
}

// At most 1024 surface columns per dimension tick and 8 maps per cadence.
// Missing columns remain unexplored: map use never generates or loads chunks.
fn sample(state: &TickState, map: &SavedMap, index: usize) -> Option<u8> {
    let step = 1_i32 << map.scale;
    let x = map.center[0] + (index as i32 % 128 - 64) * step;
    let z = map.center[1] + (index as i32 / 128 - 64) * step;
    let geometry = state.world.geometry();
    let mut counts = [0_u16; 64];
    for dx in 0..step {
        for dz in 0..step {
            for y in (geometry.min_y()..geometry.max_y()).rev() {
                let raw = state.world.block_at(x + dx, y, z + dz)?;
                let color = *state.registry.map_colors.get(raw as usize)?;
                if color != 0 {
                    counts[color as usize] += 1;
                    break;
                }
            }
        }
    }
    let color = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .max_by_key(|(_, count)| **count)
        .map_or(0, |(color, _)| color as u8);
    Some(if color == 0 { 0 } else { color * 4 + 1 })
}

pub(crate) fn tick(state: &mut TickState) {
    if !state.tick.is_multiple_of(5) {
        return;
    }
    let mut views: BTreeMap<i32, BTreeSet<ConnectionId>> = BTreeMap::new();
    let mut markers: BTreeMap<i32, Vec<(f64, f64, f32, bool)>> = BTreeMap::new();
    for (&entity, player) in &state.players {
        if player.spectator() {
            continue;
        }
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        let pos = state.entities.pos(slot);
        // Vanilla multiplayer markers: a cloned map anywhere in the
        // inventory (not only held) makes its carrier visible.
        let mut seen = std::collections::BTreeSet::new();
        for held in &player.inventory.slots {
            if held.is_empty() || !seen.insert(held.item_id) {
                continue;
            }
            if let Some(id) = map_id(held) {
                super::adopt_item_decorations(&mut state.maps, id, held);
                views.entry(id).or_default().insert(player.connection);
                markers.entry(id).or_default().push((
                    pos[0],
                    pos[2],
                    state.entities.rot(slot)[0],
                    false,
                ));
            }
        }
        let radius = f64::from(state.config_view_distance.clamp(0, 16) * 16);
        for name in ["minecraft:item_frame", "minecraft:glow_item_frame"] {
            let Some(type_id) = state
                .registry
                .entity_types
                .id_of(&mc_data::Identifier::parse(name).unwrap())
            else {
                continue;
            };
            for frame in state.entities.mob_slots(type_id) {
                if !state.entities.body(frame).alive {
                    continue;
                }
                let frame_pos = state.entities.pos(frame);
                if (frame_pos[0] - pos[0]).abs() > radius || (frame_pos[2] - pos[2]).abs() > radius
                {
                    continue;
                }
                let row = state.entities.row(frame);
                let Some(stack) = row.appearance.extra.iter().find_map(|(index, value)| {
                    if *index == 9
                        && let EntityDataValue::Item(stack) = value
                    {
                        return stack.as_ref();
                    }
                    None
                }) else {
                    continue;
                };
                let Some(id) = map_id(stack) else {
                    continue;
                };
                super::adopt_item_decorations(&mut state.maps, id, stack);
                views.entry(id).or_default().insert(player.connection);
                let angle = match row.appearance.spawn_data {
                    2 => 180.0,
                    3 => 0.0,
                    4 => 90.0,
                    5 => 270.0,
                    _ => 0.0,
                };
                let marker = (frame_pos[0].floor(), frame_pos[2].floor(), angle, true);
                let entries = markers.entry(id).or_default();
                if !entries.contains(&marker) {
                    entries.push(marker);
                }
            }
        }
    }
    let start = (state.tick / 5) as usize;
    let ids: Vec<_> = views.keys().copied().collect();
    let mut budget = 1024;
    for offset in 0..ids.len().min(8) {
        let id = ids[(start + offset) % ids.len()];
        let Some(mut map) = state.maps.records.remove(&id) else {
            continue;
        };
        // Vanilla mapping requires the map (or a clone) in a player's hand:
        // framed copies alone never sample terrain (they still deliver
        // pixels/markers to viewers). A held clone revisiting updates both.
        let holders = &markers[&id];
        let held = holders.iter().any(|&(_, _, _, frame)| !frame);
        if !map.locked && map.dimension == state.dimension && held {
            let cost = 1_usize << (2 * map.scale);
            let count = (budget / cost).min(128);
            let (x, z, _, _) = holders
                .iter()
                .enumerate()
                .filter(|(_, holder)| !holder.3)
                .nth(start % holders.iter().filter(|holder| !holder.3).count().max(1))
                .map(|(_, holder)| *holder)
                .unwrap_or(holders[start % holders.len()]);
            let step = f64::from(1_u32 << map.scale);
            let radius = (128.0 / step).ceil() as i32;
            let px = ((x - f64::from(map.center[0])) / step).floor() as i32 + 64;
            let pz = ((z - f64::from(map.center[1])) / step).floor() as i32 + 64;
            let min_x = (px - radius).clamp(0, 128) as usize;
            let max_x = (px + radius + 1).clamp(0, 128) as usize;
            let min_z = (pz - radius).clamp(0, 128) as usize;
            let max_z = (pz + radius + 1).clamp(0, 128) as usize;
            let width = max_x - min_x;
            let area = width * (max_z - min_z);
            let count = count.min(area);
            let cursor = state.maps.cursors.get(&id).copied().unwrap_or(0);
            for i in cursor..cursor + count {
                let offset = i % area;
                let index = (min_z + offset / width) * 128 + min_x + offset % width;
                if let Some(color) = sample(state, &map, index)
                    && map.colors[index] != color
                {
                    Arc::make_mut(&mut map).colors[index] = color;
                    state.maps.dirty.insert(id);
                }
            }
            state.maps.cursors.insert(
                id,
                if area == 0 {
                    0
                } else {
                    (cursor + count) % area
                },
            );
            budget -= count * cost;
        }
        for &connection in &views[&id] {
            let old = state
                .maps
                .viewers
                .insert((connection, id), (Arc::clone(&map), state.tick));
            let mut update = difference(id, &map, old.as_ref().map(|(map, _)| map.as_ref()));
            if map.tracking && map.dimension == state.dimension {
                for &(x, z, angle, frame) in &markers[&id] {
                    let distance = (x - f64::from(map.center[0]))
                        .abs()
                        .max((z - f64::from(map.center[1])).abs())
                        / f64::from(1_u32 << map.scale);
                    let kind = if frame {
                        1
                    } else if distance < 63.0 {
                        0
                    } else if distance < 320.0 {
                        6
                    } else if map.unlimited_tracking {
                        7
                    } else {
                        continue;
                    };
                    let [x, z] = map.project(x, z);
                    update.markers.push(Marker {
                        kind,
                        x,
                        z,
                        rotation: ((angle.rem_euclid(360.0) / 22.5) as u8) & 15,
                        name: None,
                    });
                }
            }
            state.push_player(connection, PlayerChange::Map { update });
        }
        state.maps.records.insert(id, map);
    }
    // Expiry is global across dimension swaps, including disconnected viewers.
    state
        .maps
        .viewers
        .retain(|_, (_, seen)| state.tick.saturating_sub(*seen) <= 20);
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn saved_maps_initial_full_and_incremental_rectangle() {
        let mut map = SavedMap::new([0, 0], 0, crate::dimensions::Dimension::Overworld).unwrap();
        let full = difference(3, &map, None);
        assert_eq!(full.rectangle, [0, 0, 128, 128]);
        assert_eq!(full.colors.len(), 16384);
        let previous = map.clone();
        map.colors[7 * 128 + 3] = 21;
        let changed = difference(3, &map, Some(&previous));
        assert_eq!(changed.rectangle, [3, 7, 1, 1]);
        assert_eq!(changed.colors, [21]);
        assert_eq!(difference(3, &map, Some(&map)).rectangle, [0; 4]);
    }
}
