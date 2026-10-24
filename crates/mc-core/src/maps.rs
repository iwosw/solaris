//! Saved maps are global native state. Items carry only the vanilla map_id.
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::boundary::{ChangeSet, EntityId, Hand};
use crate::config::CoreError;
use crate::dimensions::Dimension;
use crate::tick::TickState;
use mc_data::{Identifier, ItemStack};
use serde::{Deserialize, Serialize};

pub(crate) mod banner;
pub(crate) mod cartography;
pub(crate) mod loot;
pub mod nbt;
mod runtime;
pub(crate) mod trades;

pub const PIXELS: usize = 128 * 128;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Decoration {
    pub kind: String,
    pub x: f64,
    pub z: f64,
    pub rotation: f32,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Banner {
    pub pos: [i32; 3],
    pub color: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SavedMap {
    pub original_nbt: Vec<u8>,
    pub center: [i32; 2],
    pub scale: u8,
    pub dimension: Dimension,
    pub locked: bool,
    pub tracking: bool,
    pub unlimited_tracking: bool,
    pub colors: Vec<u8>,
    pub banners: Vec<Banner>,
    /// Retained framed waypoints (vanilla frameMarkers): emitted to
    /// held-clone viewers even beyond the live frame-view radius.
    pub frame_markers: Vec<(i32, i32, i32)>,
    pub decorations: BTreeMap<String, Decoration>,
    pub destination: Option<[i32; 3]>,
}

impl SavedMap {
    pub fn new(center: [i32; 2], scale: u8, dimension: Dimension) -> Result<Self, String> {
        if scale > 4 {
            return Err("map scale must be 0..4".into());
        }
        let span = 128_i64 << scale;
        let grid = |coordinate: i32| {
            ((i64::from(coordinate) + 64).div_euclid(span) * span + span / 2 - 64) as i32
        };
        Ok(Self {
            original_nbt: Vec::new(),
            center: center.map(grid),
            scale,
            dimension,
            locked: false,
            tracking: true,
            unlimited_tracking: false,
            colors: vec![0; PIXELS],
            banners: Vec::new(),
            frame_markers: Vec::new(),
            decorations: BTreeMap::new(),
            destination: None,
        })
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.scale > 4
            || self.colors.len() != PIXELS
            || self.colors.iter().any(|c| *c >= 248)
            || self.banners.len() > 256
            || self.decorations.len() > 256
            || self.decorations.values().any(|d| {
                !d.x.is_finite()
                    || !d.z.is_finite()
                    || !d.rotation.is_finite()
                    || decoration_id(&d.kind).is_none()
            })
        {
            return Err("invalid saved map".into());
        }
        Ok(())
    }

    /// Map-space coordinates are half-pixels, clamped at the display border.
    pub fn project(&self, x: f64, z: f64) -> [i8; 2] {
        let unit = f64::from(1_u32 << self.scale);
        [x - f64::from(self.center[0]), z - f64::from(self.center[1])]
            .map(|n| (n / unit * 2.0 + 0.5).floor().clamp(-128.0, 127.0) as i8)
    }
}

/// Global monotonic allocation fence, including imported idcounts gaps.
#[derive(Debug, Default)]
pub struct Maps {
    pub records: BTreeMap<i32, Arc<SavedMap>>,
    pub next_id: i64,
    pub(crate) dirty: BTreeSet<i32>,
    pub(crate) cursors: BTreeMap<i32, usize>,
    pub(crate) viewers: BTreeMap<(crate::boundary::ConnectionId, i32), (Arc<SavedMap>, u64)>,
    pub(crate) pending_loot: BTreeMap<(Dimension, [i32; 3]), loot::Pending>,
    pub(crate) pending_trades: BTreeMap<EntityId, trades::Pending>,
    pub(crate) attempted_trades: BTreeSet<(uuid::Uuid, u8)>,
}
impl Maps {
    pub fn allocate(&mut self, map: SavedMap) -> Result<i32, String> {
        map.validate()?;
        let id = i32::try_from(self.next_id).map_err(|_| "map ids exhausted")?;
        self.next_id += 1;
        self.records.insert(id, Arc::new(map));
        self.dirty.insert(id);
        Ok(id)
    }
    pub(crate) fn restore(
        records: BTreeMap<i32, Arc<SavedMap>>,
        floor: i64,
    ) -> Result<Self, CoreError> {
        for (&id, map) in &records {
            if id < 0 {
                return Err(CoreError::Storage("negative saved map id".into()));
            }
            map.validate().map_err(CoreError::Storage)?;
        }
        let next_id = floor.max(
            records
                .last_key_value()
                .map_or(0, |(&id, _)| i64::from(id) + 1),
        );
        if !(0..=i64::from(i32::MAX) + 1).contains(&next_id) {
            return Err(CoreError::Storage("invalid map sequence".into()));
        }
        Ok(Self {
            records,
            next_id,
            ..Self::default()
        })
    }
}

pub fn map_id(stack: &ItemStack) -> Option<i32> {
    let component = stack
        .extended()
        .preserved_components
        .iter()
        .find(|c| c.name == "minecraft:map_id")?;
    let mut bytes = component.value.as_slice();
    match mc_nbt::read_network_value(&mut bytes).ok()? {
        mc_nbt::Tag::Int(id) if id >= 0 && bytes.is_empty() => Some(id),
        _ => None,
    }
}
pub fn set_map_id(stack: &mut ItemStack, id: i32) {
    let mut value = Vec::new();
    mc_nbt::write_network_value(&mut value, &mc_nbt::Tag::Int(id)).expect("map id NBT");
    let components = &mut stack.extended_mut().preserved_components;
    components.retain(|c| c.name.trim_start_matches('!') != "minecraft:map_id");
    components.push(mc_data::item_stack::PreservedItemComponent {
        name: "minecraft:map_id".into(),
        value,
        baseline: None,
    });
}

pub fn filled_map(state: &TickState, id: i32) -> Result<ItemStack, String> {
    let item = state
        .registry
        .items
        .id_of(&Identifier::parse("minecraft:filled_map").unwrap())
        .ok_or("filled_map item missing")?;
    let mut stack = ItemStack::new(item, 1);
    set_map_id(&mut stack, id);
    Ok(stack)
}

/// Call after a structure planner has returned a real destination. No synthetic
/// marker or unexplored-chunk claim is invented by the map owner.
pub fn create_exploration_map(
    state: &mut TickState,
    destination: [i32; 3],
    dimension: Dimension,
    decoration: &str,
    scale: u8,
) -> Result<ItemStack, String> {
    let map = exploration(destination, dimension, decoration, scale)?;
    let id = i32::try_from(state.maps.next_id).map_err(|_| "map ids exhausted")?;
    let mut stack = filled_map(state, id)?;
    attach_decorations(&mut stack, &map);
    state.maps.allocate(map)?;
    Ok(stack)
}

fn exploration(
    destination: [i32; 3],
    dimension: Dimension,
    decoration: &str,
    scale: u8,
) -> Result<SavedMap, String> {
    if decoration_id(decoration).is_none() {
        return Err("unknown map decoration".into());
    }
    let mut map = SavedMap::new([destination[0], destination[2]], scale, dimension)?;
    map.destination = Some(destination);
    map.unlimited_tracking = true;
    map.decorations.insert(
        "destination".into(),
        Decoration {
            kind: decoration.into(),
            x: f64::from(destination[0]),
            z: f64::from(destination[2]),
            rotation: 180.0,
            name: None,
        },
    );
    Ok(map)
}

pub(crate) fn use_empty(
    state: &mut TickState,
    actor: EntityId,
    hand: Hand,
    changes: &mut ChangeSet,
) -> bool {
    let Some(player) = state.players.get(&actor) else {
        return false;
    };
    let held = &player.inventory.slots[crate::tick::held_slot_index(player, hand)];
    if held.is_empty()
        || state
            .registry
            .items
            .name_of(held.item_id)
            .is_none_or(|id| id.as_str() != "minecraft:map")
    {
        return false;
    }
    if player.spectator() || crate::plugin_inventories::has_reservations_for_player(state, actor) {
        return true;
    }
    let connection = player.connection;
    let Some(slot) = state.entities.slot_of(actor) else {
        return true;
    };
    let pos = state.entities.pos(slot);
    let Ok(map) = SavedMap::new(
        [pos[0].floor() as i32, pos[2].floor() as i32],
        0,
        state.dimension,
    ) else {
        return true;
    };
    let Ok(id) = state.maps.allocate(map) else {
        return true;
    };
    let product = filled_map(state, id).expect("vanilla filled_map");
    let player = state.players.get_mut(&actor).unwrap();
    let rules = state.registry.item_rules();
    let effects = if player.creative() {
        player.receive_stack(&rules, product)
    } else {
        player.harvest_held(&rules, hand, product, true)
    };
    crate::tick::apply_player_effects(state, connection, actor, effects, changes);
    true
}

pub(crate) use runtime::tick;
pub use runtime::{MapUpdate, Marker};

// Local reports/registries.json, minecraft:map_decoration_type, 26.1.2.
pub fn decoration_id(name: &str) -> Option<i32> {
    const NAMES: [&str; 35] = [
        "player",
        "frame",
        "red_marker",
        "blue_marker",
        "target_x",
        "target_point",
        "player_off_map",
        "player_off_limits",
        "mansion",
        "monument",
        "banner_white",
        "banner_orange",
        "banner_magenta",
        "banner_light_blue",
        "banner_yellow",
        "banner_lime",
        "banner_pink",
        "banner_gray",
        "banner_light_gray",
        "banner_cyan",
        "banner_purple",
        "banner_blue",
        "banner_brown",
        "banner_green",
        "banner_red",
        "banner_black",
        "red_x",
        "village_desert",
        "village_plains",
        "village_savanna",
        "village_snowy",
        "village_taiga",
        "jungle_temple",
        "swamp_hut",
        "trial_chambers",
    ];
    NAMES
        .iter()
        .position(|n| *n == name.strip_prefix("minecraft:").unwrap_or(name))
        .map(|i| i as i32)
}

pub(crate) fn checkpoint(
    records: &BTreeMap<i32, Arc<SavedMap>>,
    tick: u64,
    globals: &mut BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Result<(), CoreError> {
    for (&id, map) in records {
        globals.insert(
            format!("map/{id}"),
            crate::native_checkpoint::record(
                tick,
                serde_json::to_vec(map).map_err(|e| CoreError::Storage(e.to_string()))?,
            ),
        );
    }
    let next = sequence(globals)?.max(
        records
            .last_key_value()
            .map_or(0, |(&id, _)| i64::from(id) + 1),
    );
    globals.insert(
        "map_sequence".into(),
        crate::native_checkpoint::record(
            tick,
            serde_json::to_vec(&next).map_err(|e| CoreError::Storage(e.to_string()))?,
        ),
    );
    Ok(())
}
pub(crate) fn recover(
    globals: &BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Result<BTreeMap<i32, Arc<SavedMap>>, CoreError> {
    let mut maps = BTreeMap::new();
    for (key, record) in globals {
        let Some(id) = key.strip_prefix("map/") else {
            continue;
        };
        let id: i32 = id
            .parse()
            .map_err(|_| CoreError::Storage("invalid map key".into()))?;
        if id < 0 || record.version != 1 {
            return Err(CoreError::Storage("invalid map record".into()));
        }
        let map: SavedMap = serde_json::from_slice(&record.payload)
            .map_err(|e| CoreError::Storage(e.to_string()))?;
        map.validate().map_err(CoreError::Storage)?;
        maps.insert(id, Arc::new(map));
    }
    Ok(maps)
}

pub(crate) fn sequence(
    globals: &BTreeMap<String, mc_world::native::NativeDomainRecord>,
) -> Result<i64, CoreError> {
    let Some(record) = globals.get("map_sequence") else {
        return Ok(0);
    };
    let next: i64 =
        serde_json::from_slice(&record.payload).map_err(|e| CoreError::Storage(e.to_string()))?;
    if record.version != 1 || !(0..=i64::from(i32::MAX) + 1).contains(&next) {
        return Err(CoreError::Storage("invalid map sequence".into()));
    }
    Ok(next)
}

fn attach_decorations(stack: &mut ItemStack, map: &SavedMap) {
    use mc_nbt::Tag;
    let decorations = map
        .decorations
        .iter()
        .map(|(key, d)| {
            (
                key.clone(),
                Tag::Compound(vec![
                    ("type".into(), Tag::String(d.kind.clone())),
                    ("x".into(), Tag::Double(d.x)),
                    ("z".into(), Tag::Double(d.z)),
                    ("rotation".into(), Tag::Float(d.rotation)),
                ]),
            )
        })
        .collect();
    let mut value = Vec::new();
    mc_nbt::write_network_value(&mut value, &Tag::Compound(decorations))
        .expect("map decoration NBT");
    stack
        .extended_mut()
        .preserved_components
        .push(mc_data::item_stack::PreservedItemComponent {
            name: "minecraft:map_decorations".into(),
            value,
            baseline: None,
        });
}

fn adopt_item_decorations(maps: &mut Maps, id: i32, stack: &ItemStack) {
    let Some(map) = maps.records.get_mut(&id) else {
        return;
    };
    if !map.decorations.is_empty() {
        return;
    }
    let Some(component) = stack
        .extended()
        .preserved_components
        .iter()
        .find(|c| c.name == "minecraft:map_decorations")
    else {
        return;
    };
    use mc_nbt::Tag;
    let Ok(Tag::Compound(entries)) = mc_nbt::read_network_value(&mut component.value.as_slice())
    else {
        return;
    };
    for (key, value) in entries.into_iter().take(256) {
        let Tag::Compound(fields) = value else {
            continue;
        };
        let field = |key: &str| fields.iter().find(|(k, _)| k == key).map(|(_, v)| v);
        let (
            Some(Tag::String(kind)),
            Some(Tag::Double(x)),
            Some(Tag::Double(z)),
            Some(Tag::Float(rotation)),
        ) = (field("type"), field("x"), field("z"), field("rotation"))
        else {
            continue;
        };
        if decoration_id(kind).is_none()
            || !x.is_finite()
            || !z.is_finite()
            || !rotation.is_finite()
        {
            continue;
        }
        Arc::make_mut(map).decorations.insert(
            key,
            Decoration {
                kind: kind.clone(),
                x: *x,
                z: *z,
                rotation: *rotation,
                name: None,
            },
        );
        maps.dirty.insert(id);
    }
}
