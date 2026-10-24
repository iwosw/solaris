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
    // Vanilla ceiling dimensions (Nether) map a generated dirt/stone noise
    // pattern, not the physical roof terrain.
    if map.dimension == crate::dimensions::Dimension::Nether {
        return Some(nether_sample(state, map, index));
    }
    let step = 1_i32 << map.scale;
    let x = map.center[0] + (index as i32 % 128 - 64) * step;
    let z = map.center[1] + (index as i32 / 128 - 64) * step;
    let geometry = state.world.geometry();
    // Per-column surface color + height + water depth for brightness.
    let mut cells: Vec<(u8, i32, u32)> = Vec::new();
    for dx in 0..step {
        for dz in 0..step {
            let mut water_depth = 0_u32;
            for y in (geometry.min_y()..geometry.max_y()).rev() {
                let raw = state.world.block_at(x + dx, y, z + dz)?;
                let color = *state.registry.map_colors.get(raw as usize)?;
                if color == 0 {
                    continue;
                }
                let water = state.registry.materials.classify(raw)
                    == mc_physics::BlockMaterial::Water;
                if water {
                    water_depth += 1;
                    continue;
                }
                cells.push((color, y, water_depth));
                break;
            }
        }
    }
    if cells.is_empty() {
        return Some(0);
    }
    // Majority color wins (vanilla multiset); average height/depth decide
    // the brightness of the winning color.
    let mut counts = [0_u16; 64];
    for (color, _, _) in &cells {
        counts[*color as usize] += 1;
    }
    let color = counts
        .iter()
        .enumerate()
        .filter(|(_, count)| **count != 0)
        .max_by_key(|(_, count)| **count)
        .map_or(0, |(color, _)| color as u8);
    let (height_sum, depth_sum, n) = cells
        .iter()
        .filter(|(c, _, _)| *c == color)
        .fold((0_i64, 0_u64, 0_u64), |(h, d, n), (_, y, depth)| {
            (h + i64::from(*y), d + u64::from(*depth), n + 1)
        });
    let height = height_sum / n.max(1) as i64;
    let depth = (depth_sum / n.max(1) as u64).min(8) as i32;
    // Vanilla brightness: water depth darkens (depth>0), north height
    // difference shades slopes, flat dry land stays base+1.
    let north = north_height(state, map, index, step);
    let shade = if depth > 0 {
        // Deeper water renders darker; shallow water keeps base brightness.
        if depth >= 3 { 0 } else { 1 }
    } else if height > north {
        2
    } else if height < north {
        0
    } else {
        1
    };
    Some(if color == 0 { 0 } else { color * 4 + shade as u8 })
}

/// Average surface height of the pixel row north of this pixel (for slope
/// shading). Missing columns reuse the pixel's own height (flat).
fn north_height(state: &TickState, map: &SavedMap, index: usize, step: i32) -> i64 {
    let col = index % 128;
    let row = index / 128;
    if row == 0 {
        return 0;
    }
    let geometry = state.world.geometry();
    let nx = map.center[0] + (col as i32 - 64) * step;
    let nz = map.center[1] + ((row - 1) as i32 - 64) * step;
    let mut sum = 0_i64;
    let mut n = 0_i64;
    for dx in 0..step {
        for dz in 0..step {
            for y in (geometry.min_y()..geometry.max_y()).rev() {
                let Some(raw) = state.world.block_at(nx + dx, y, nz + dz) else {
                    continue;
                };
                let Some(color) = state.registry.map_colors.get(raw as usize) else {
                    continue;
                };
                if *color == 0 {
                    continue;
                }
                if state.registry.materials.classify(raw) == mc_physics::BlockMaterial::Water {
                    continue;
                }
                sum += i64::from(y);
                n += 1;
                break;
            }
        }
    }
    if n == 0 { 0 } else { sum / n }
}

/// Vanilla ceiling-dimension pattern: pseudo-random dirt/stone map colors
/// from the world seed and pixel position, not physical blocks.
fn nether_sample(state: &TickState, map: &SavedMap, index: usize) -> u8 {
    let step = 1_i32 << map.scale;
    let x = map.center[0] + (index as i32 % 128 - 64) * step;
    let z = map.center[1] + (index as i32 / 128 - 64) * step;
    let mut seed = (state.world.seed() as u64)
        .wrapping_add((x as u64) << 32 | (z as u32 as u64))
        .wrapping_add(index as u64);
    seed ^= seed >> 30;
    seed = seed.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    seed ^= seed >> 27;
    seed = seed.wrapping_mul(0x94D0_49BB_1331_11EB);
    seed ^= seed >> 31;
    // Dirt/stone map colors with brightness variation (vanilla-like noise).
    let base = if seed & 1 == 0 { 13_u8 } else { 11_u8 };
    base * 4 + ((seed >> 7) % 3) as u8
}

pub(crate) fn tick(state: &mut TickState) {
    if !state.tick.is_multiple_of(5) {
        return;
    }
    let mut views: BTreeMap<i32, BTreeSet<ConnectionId>> = BTreeMap::new();
    // Marker: x, z, rotation, frame flag, carrier-hidden, owner connection.
    let mut markers: BTreeMap<i32, Vec<(f64, f64, f32, bool, bool, ConnectionId)>> =
        BTreeMap::new();
    for (&entity, player) in &state.players {
        if player.spectator() {
            continue;
        }
        let Some(slot) = state.entities.slot_of(entity) else {
            continue;
        };
        let pos = state.entities.pos(slot);
        // Vanilla pumpkin hiding: a carrier wearing a carved pumpkin is
        // hidden from other players' maps (but keeps their own marker).
        let hidden = player.inventory.slots.get(5).is_some_and(|stack| {
            !stack.is_empty()
                && state
                    .registry
                    .items
                    .name_of(stack.item_id)
                    .is_some_and(|id| id.as_str() == "minecraft:carved_pumpkin")
        });
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
                    hidden,
                    player.connection,
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
                let marker = (
                    frame_pos[0].floor(),
                    frame_pos[2].floor(),
                    angle,
                    true,
                    false,
                    player.connection,
                );
                let entries = markers.entry(id).or_default();
                if !entries.contains(&marker) {
                    entries.push(marker);
                }
                // Retain the framed waypoint on the map itself so distant
                // held clones keep the green pointer after everyone leaves
                // the frame-view radius (vanilla frameMarkers).
                if let Some(map) = state.maps.records.get_mut(&id) {
                    let map = std::sync::Arc::make_mut(map);
                    let retained = (
                        frame_pos[0].floor() as i32,
                        frame_pos[2].floor() as i32,
                        angle as i32,
                    );
                    if !map.frame_markers.contains(&retained) {
                        map.frame_markers.push(retained);
                        state.maps.dirty.insert(id);
                    }
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
        // Vanilla checkBanners: destroyed/changed banners lose their markers
        // when the area remaps. Validate stored banners against loaded world
        // state before sampling.
        if !map.locked && map.dimension == state.dimension {
            let stale: Vec<usize> = map
                .banners
                .iter()
                .enumerate()
                .filter(|(_, banner)| {
                    let [x, y, z] = banner.pos;
                    // Only loaded cells can evict; distant/unloaded markers persist.
                    let Some(current) = state.world.block_at(x, y, z) else {
                        return false;
                    };
                    let Some(block) = state.registry.blocks.by_id(mc_world::BlockStateId(current))
                    else {
                        return true;
                    };
                    let path = block.block.id.path();
                    let color = path
                        .strip_suffix("_wall_banner")
                        .or_else(|| path.strip_suffix("_banner"));
                    match color {
                        Some(color) if color == banner.color => false,
                        _ => true,
                    }
                })
                .map(|(index, _)| index)
                .collect();
            if !stale.is_empty() {
                let kept = Arc::make_mut(&mut map);
                for index in stale.into_iter().rev() {
                    kept.banners.remove(index);
                }
                state.maps.dirty.insert(id);
            }
        }
        // Vanilla mapping requires the map (or a clone) in a player's hand:
        // framed copies alone never sample terrain (they still deliver
        // pixels/markers to viewers). A held clone revisiting updates both.
        let holders = &markers[&id];
        let held = holders.iter().any(|&(_, _, _, frame, _, _)| !frame);
        if !map.locked && map.dimension == state.dimension && held {
            let cost = 1_usize << (2 * map.scale);
            let count = (budget / cost).min(128);
            let (x, z, _, _, _, _) = holders
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
                // Live markers first (frame + carriers), then retained framed
                // waypoints not currently visible (distant home waypoints).
                let live: std::collections::BTreeSet<(i64, i64)> = markers[&id]
                    .iter()
                    .map(|&(x, z, _, _, _, _)| (x as i64, z as i64))
                    .collect();
                let mut all = markers[&id].clone();
                for &(fx, fz, angle) in &map.frame_markers {
                    let (fx, fz) = (fx as f64, fz as f64);
                    if live.contains(&(fx as i64, fz as i64)) {
                        continue;
                    }
                    all.push((fx, fz, angle as f32, true, false, connection));
                }
                for &(x, z, angle, frame, hidden, owner) in &all {
                    // Pumpkin hiding is recipient-dependent: hidden carriers
                    // stay visible on their own map, not on others'.
                    if hidden && owner != connection {
                        continue;
                    }
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
