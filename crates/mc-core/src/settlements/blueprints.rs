//! Solaris-authored settlement blueprints (vanilla blocks only).
//!
//! Data lives in `assets/blueprints/*.json` as readable ASCII layers (one string per
//! z row, one char per x, `.` = air, layers listed bottom-up). Each level is the
//! complete picture of that level; the loader derives the upgrade diff against the
//! previous level (blocks to place, blocks to clear), the material list, the four
//! pre-rotated placements and the certification box.
//!
//! Frame: offsets are relative to `origin`, the min corner of the rotated maximum
//! footprint box ([`Blueprint::footprint`]). `origin.y` is the ground surface block;
//! the floor layer (y offset 0) replaces it and the interior starts one above.
//! The certification box is `cert` (full x/z of the cert footprint, floor up to the
//! ceiling layer); roof crowns above it and yards beside it are outside the box.
use std::collections::BTreeMap;
use std::sync::OnceLock;

use mc_data::Identifier;
use mc_script::ScriptWorldBounds;
use mc_world::BlockStateId;
use serde::Deserialize;

use super::buildings::BuildingKind;
use crate::registry::CoreRegistry;

/// Protocol item id.
pub type ItemId = u32;

/// Placed size limit `[x, y, z]` for every blueprint. Villager builders place within
/// reach of a ground-level stand point, so nothing may rise above y offset 8.
pub const MAX_SIZE: [u8; 3] = [11, 9, 11];

const SOURCES: [&str; 9] = [
    include_str!("../../assets/blueprints/house.json"),
    include_str!("../../assets/blueprints/storage.json"),
    include_str!("../../assets/blueprints/farm.json"),
    include_str!("../../assets/blueprints/mason.json"),
    include_str!("../../assets/blueprints/lumber_camp.json"),
    include_str!("../../assets/blueprints/mine.json"),
    include_str!("../../assets/blueprints/barracks.json"),
    include_str!("../../assets/blueprints/wall.json"),
    include_str!("../../assets/blueprints/watch_tower.json"),
];
const DIRS: [&str; 4] = ["north", "east", "south", "west"];

/// Clockwise quarter turns seen from above.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rotation(pub u8);
impl Rotation {
    pub const ALL: [Self; 4] = [Self(0), Self(1), Self(2), Self(3)];
    #[must_use]
    pub fn from_quarter_turns(turns: u8) -> Self {
        Self(turns % 4)
    }
    fn index(self) -> usize {
        usize::from(self.0 % 4)
    }
}

#[derive(Debug, Clone)]
pub struct Blueprint {
    pub id: String,
    pub kind: BuildingKind,
    /// `levels[0]` is level 1.
    pub levels: Vec<BlueprintLevel>,
    /// Maximum placed extent `[x, y, z]` over all levels, unrotated.
    extent: [u8; 3],
}
impl Blueprint {
    /// 1-based level lookup.
    #[must_use]
    pub fn level(&self, level: u8) -> Option<&BlueprintLevel> {
        self.levels.get(usize::from(level.checked_sub(1)?))
    }
    /// Maximum placed extent over all levels in the rotated frame; origin is its min corner.
    #[must_use]
    pub fn footprint(&self, rotation: Rotation) -> [u8; 3] {
        let [x, y, z] = self.extent;
        if rotation.0 % 2 == 1 {
            [z, y, x]
        } else {
            [x, y, z]
        }
    }
}

#[derive(Debug, Clone)]
pub struct BlueprintLevel {
    /// Placed extent of this level, unrotated.
    pub size: [u8; 3],
    /// Certification box extent (starts at offset zero), unrotated.
    pub cert: [u8; 3],
    /// Diff vs the previous level (whole level for L1), placement order.
    pub blocks: Vec<([i8; 3], BlockStateId)>,
    /// Cells to clear vs the previous level; apply before `blocks`.
    pub removals: Vec<[i8; 3]>,
    /// Item totals for `blocks` (door/bed halves counted once, double slabs twice).
    pub materials: Vec<(ItemId, u32)>,
    pub entrance: [i8; 3],
    rotated: [RotatedLevel; 4],
}
impl BlueprintLevel {
    #[must_use]
    pub fn rotated(&self, rotation: Rotation) -> &RotatedLevel {
        &self.rotated[rotation.index()]
    }
}

#[derive(Debug, Clone)]
pub struct RotatedLevel {
    pub blocks: Vec<([i8; 3], BlockStateId)>,
    pub removals: Vec<[i8; 3]>,
    pub size: [u8; 3],
    pub entrance: [i8; 3],
    cert_min: [i8; 3],
    cert_max: [i8; 3],
}
impl RotatedLevel {
    #[must_use]
    pub fn world(&self, origin: [i32; 3], offset: [i8; 3]) -> [i32; 3] {
        std::array::from_fn(|i| origin[i] + i32::from(offset[i]))
    }
    /// Certification bounds for this level at `origin`.
    #[must_use]
    pub fn bounds(&self, origin: [i32; 3]) -> ScriptWorldBounds {
        ScriptWorldBounds::new(
            self.world(origin, self.cert_min),
            self.world(origin, self.cert_max),
        )
        .expect("blueprint certification box is a valid bounds")
    }
}

/// All blueprints. Block-state ids are stable for one vanilla data version, so the
/// first registry resolves them for the process.
#[must_use]
pub fn all(registry: &CoreRegistry) -> &'static [Blueprint] {
    static CACHE: OnceLock<Vec<Blueprint>> = OnceLock::new();
    CACHE.get_or_init(|| load(registry).expect("embedded blueprints are valid"))
}
#[must_use]
pub fn get(registry: &CoreRegistry, id: &str) -> Option<&'static Blueprint> {
    all(registry).iter().find(|b| b.id == id)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBlueprint {
    id: String,
    kind: String,
    palette: BTreeMap<char, String>,
    levels: Vec<RawLevel>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawLevel {
    cert: [u8; 3],
    entrance: [i8; 3],
    /// Bottom-up layers; each layer is z rows of x chars.
    layers: Vec<Vec<String>>,
}

type Cell = Option<BlockStateId>;
struct Grid {
    size: [usize; 3],
    cells: Vec<Cell>,
}
impl Grid {
    fn at(&self, x: i32, y: i32, z: i32) -> Cell {
        let [sx, sy, sz] = self.size.map(|s| s as i32);
        if x < 0 || y < 0 || z < 0 || x >= sx || y >= sy || z >= sz {
            return None;
        }
        self.cells[(y as usize * self.size[2] + z as usize) * self.size[0] + x as usize]
    }
    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (y * self.size[2] + z) * self.size[0] + x
    }
}

/// Parses `name[key=value,...]` against the block registry.
fn parse_state(registry: &CoreRegistry, text: &str) -> Result<BlockStateId, String> {
    let (name, props) = match text.split_once('[') {
        Some((name, rest)) => (name, rest.strip_suffix(']').ok_or("missing ]")?),
        None => (text, ""),
    };
    let name = if name.contains(':') {
        name.to_owned()
    } else {
        format!("minecraft:{name}")
    };
    let id = Identifier::parse(name.as_str()).map_err(|_| format!("bad block id {name}"))?;
    let block = registry
        .blocks
        .block(&id)
        .ok_or_else(|| format!("unknown block {name}"))?;
    let mut full = registry
        .blocks
        .by_id(block.default)
        .ok_or("no default state")?
        .properties
        .clone();
    for pair in props.split(',').filter(|p| !p.is_empty()) {
        let (key, value) = pair.split_once('=').ok_or("property needs key=value")?;
        let entry = full
            .iter_mut()
            .find(|(k, _)| k == key)
            .ok_or_else(|| format!("{name} has no property {key}"))?;
        entry.1 = value.to_owned();
    }
    registry
        .blocks
        .by_name_and_props(&id, &full)
        .ok_or_else(|| format!("invalid state {text}"))
}

fn state_parts(registry: &CoreRegistry, id: BlockStateId) -> (String, Vec<(String, String)>) {
    let def = registry.blocks.by_id(id).expect("known state");
    (def.block.id.to_string(), def.properties.clone())
}
fn path_of(registry: &CoreRegistry, id: BlockStateId) -> &str {
    registry
        .blocks
        .by_id(id)
        .expect("known state")
        .block
        .id
        .path()
}
fn with_props(
    registry: &CoreRegistry,
    id: BlockStateId,
    props: &[(String, String)],
) -> Result<BlockStateId, String> {
    let def = registry.blocks.by_id(id).ok_or("unknown state")?;
    registry
        .blocks
        .by_name_and_props(&def.block.id, props)
        .ok_or_else(|| format!("invalid state of {}: {props:?}", def.block.id))
}

fn full_block(registry: &CoreRegistry, id: BlockStateId) -> bool {
    registry.collision_boxes[id.0 as usize].as_ref() == [mc_physics::BlockCollisionBox::FULL_BLOCK]
}

/// Fence/pane/wall families connect to their own kind and to full blocks.
fn connection_family(path: &str) -> Option<&'static str> {
    if path.ends_with("_fence") {
        Some("fence")
    } else if path.ends_with("_pane") || path == "iron_bars" {
        Some("pane")
    } else if path.ends_with("_wall") {
        Some("wall")
    } else {
        None
    }
}
fn connects(registry: &CoreRegistry, family: &str, neighbor: Cell) -> bool {
    let Some(id) = neighbor else { return false };
    let path = path_of(registry, id);
    full_block(registry, id)
        || match family {
            "fence" => path.ends_with("_fence") || path.ends_with("_fence_gate"),
            _ => {
                connection_family(path).is_some_and(|f| f != "fence")
                    || path.ends_with("_fence_gate")
            }
        }
}
fn auto_connect(registry: &CoreRegistry, grid: &mut Grid) -> Result<(), String> {
    let deltas = [(0, -1), (1, 0), (0, 1), (-1, 0)];
    for y in 0..grid.size[1] {
        for z in 0..grid.size[2] {
            for x in 0..grid.size[0] {
                let Some(id) = grid.cells[grid.index(x, y, z)] else {
                    continue;
                };
                let Some(family) = connection_family(path_of(registry, id)) else {
                    continue;
                };
                let (_, mut props) = state_parts(registry, id);
                let mut count = 0;
                let mut flags = [false; 4];
                for (i, (dx, dz)) in deltas.iter().enumerate() {
                    let neighbor = grid.at(x as i32 + dx, y as i32, z as i32 + dz);
                    flags[i] = connects(registry, family, neighbor);
                    count += usize::from(flags[i]);
                }
                for (i, dir) in DIRS.iter().enumerate() {
                    let value = match (family, flags[i]) {
                        ("wall", true) => "low",
                        ("wall", false) => "none",
                        (_, true) => "true",
                        (_, false) => "false",
                    };
                    if let Some(entry) = props.iter_mut().find(|(k, _)| k == dir) {
                        entry.1 = value.to_owned();
                    }
                }
                if family == "wall" {
                    let straight = count == 2 && (flags[0] == flags[2]);
                    if let Some(entry) = props.iter_mut().find(|(k, _)| k == "up") {
                        entry.1 = (!straight).to_string();
                    }
                }
                let index = grid.index(x, y, z);
                grid.cells[index] = Some(with_props(registry, id, &props)?);
            }
        }
    }
    Ok(())
}

fn attachment(path: &str) -> bool {
    path.ends_with("torch")
        || path.ends_with("ladder")
        || path.ends_with("lantern")
        || path.ends_with("trapdoor")
        || path.ends_with("button")
        || path.ends_with("pressure_plate")
        || path.ends_with("carpet")
        || path.ends_with("rail")
        || path.ends_with("_sign")
        || path.ends_with("banner")
        || path.ends_with("vine")
}

/// Item (and count) one placed block costs; `None` = free half or air.
fn block_cost(registry: &CoreRegistry, id: BlockStateId) -> Result<Option<(ItemId, u32)>, String> {
    let def = registry.blocks.by_id(id).ok_or("unknown state")?;
    let path = def.block.id.path();
    let prop = |key: &str, value: &str| def.properties.iter().any(|(k, v)| k == key && v == value);
    if matches!(path, "air" | "cave_air" | "void_air")
        || (prop("half", "upper")
            && (path.ends_with("_door")
                || path.contains("tall")
                || path.ends_with("peony")
                || path.ends_with("lilac")))
        || (path.ends_with("_bed") && prop("part", "head"))
    {
        return Ok(None);
    }
    let item = match path {
        "wall_torch" => "torch",
        "soul_wall_torch" => "soul_torch",
        "redstone_wall_torch" => "redstone_torch",
        "wheat" => "wheat_seeds",
        "carrots" => "carrot",
        "potatoes" => "potato",
        "beetroots" => "beetroot_seeds",
        "farmland" => "dirt",
        "water" => "water_bucket",
        "lava" => "lava_bucket",
        "redstone_wire" => "redstone",
        other => match other.strip_suffix("_wall_sign") {
            Some(wood) => return item_cost(registry, &format!("{wood}_sign"), 1),
            None => other,
        },
    };
    let count = if prop("type", "double") { 2 } else { 1 };
    item_cost(registry, item, count)
}
/// Item cost of one placed block state (`None` = free half, air or no item).
#[must_use]
pub fn material_for(registry: &CoreRegistry, state: BlockStateId) -> Option<(ItemId, u32)> {
    block_cost(registry, state).ok().flatten()
}
fn item_cost(
    registry: &CoreRegistry,
    item: &str,
    count: u32,
) -> Result<Option<(ItemId, u32)>, String> {
    let id =
        Identifier::parse(format!("minecraft:{item}")).map_err(|_| format!("bad item {item}"))?;
    let item = registry
        .items
        .id_of(&id)
        .ok_or_else(|| format!("block has no item {id}"))?;
    Ok(Some((item, count)))
}

fn rotate_dir(value: &str, turns: usize) -> Option<&'static str> {
    DIRS.iter()
        .position(|d| *d == value)
        .map(|i| DIRS[(i + turns) % 4])
}
fn rotate_state(
    registry: &CoreRegistry,
    id: BlockStateId,
    turns: usize,
) -> Result<BlockStateId, String> {
    if turns == 0 {
        return Ok(id);
    }
    let (_, props) = state_parts(registry, id);
    let path = path_of(registry, id);
    let mut out: Vec<(String, String)> = Vec::with_capacity(props.len());
    for (key, value) in &props {
        let (key, value) = match key.as_str() {
            "facing" => (
                key.clone(),
                rotate_dir(value, turns).unwrap_or(value).to_owned(),
            ),
            "axis" if turns % 2 == 1 && value == "x" => (key.clone(), "z".to_owned()),
            "axis" if turns % 2 == 1 && value == "z" => (key.clone(), "x".to_owned()),
            "north" | "east" | "south" | "west" => (
                rotate_dir(key, turns).expect("direction").to_owned(),
                value.clone(),
            ),
            "rotation" => {
                let n: usize = value.parse().map_err(|_| "bad rotation")?;
                (key.clone(), ((n + 4 * turns) % 16).to_string())
            }
            "shape" if path.ends_with("rail") => {
                let tokens: Vec<String> = value
                    .split('_')
                    .map(|t| {
                        rotate_dir(t, turns)
                            .unwrap_or(if t == "ascending" { "ascending" } else { t })
                            .to_owned()
                    })
                    .collect();
                (key.clone(), tokens.join("_"))
            }
            _ => (key.clone(), value.clone()),
        };
        out.push((key, value));
    }
    if let Some(shape) = out.iter().position(|(k, _)| k == "shape")
        && path.ends_with("rail")
    {
        // Two-token corner names are canonical in one order only.
        let value = out[shape].1.clone();
        let def = registry.blocks.by_id(id).ok_or("unknown state")?;
        let allowed = def
            .block
            .properties
            .iter()
            .find(|(k, _)| k == "shape")
            .map(|(_, v)| v.clone())
            .unwrap_or_default();
        if !allowed.contains(&value) {
            let swapped: Vec<&str> = value.split('_').rev().collect();
            out[shape].1 = swapped.join("_");
        }
    }
    with_props(registry, id, &out)
}

fn rotate_xz(turns: usize, x: i32, z: i32, fx: i32, fz: i32) -> (i32, i32) {
    match turns % 4 {
        0 => (x, z),
        1 => (fz - 1 - z, x),
        2 => (fx - 1 - x, fz - 1 - z),
        _ => (z, fx - 1 - x),
    }
}
fn narrow(value: i32) -> Result<i8, String> {
    i8::try_from(value).map_err(|_| "offset exceeds i8".to_owned())
}

#[allow(clippy::too_many_lines)]
fn load_blueprint(registry: &CoreRegistry, raw: RawBlueprint) -> Result<Blueprint, String> {
    let kind =
        BuildingKind::parse(&raw.kind).ok_or_else(|| format!("unknown kind {}", raw.kind))?;
    let mut palette = BTreeMap::new();
    for (ch, state) in &raw.palette {
        palette.insert(
            *ch,
            parse_state(registry, state).map_err(|e| format!("{}: palette {ch}: {e}", raw.id))?,
        );
    }
    let mut grids = Vec::new();
    for (n, level) in raw.levels.iter().enumerate() {
        let ctx = |e: String| format!("{} level {}: {e}", raw.id, n + 1);
        let sy = level.layers.len();
        let sz = level.layers.first().map_or(0, Vec::len);
        let sx = level
            .layers
            .first()
            .and_then(|l| l.first())
            .map_or(0, |r| r.chars().count());
        if sx == 0 || sy == 0 || sz == 0 {
            return Err(ctx("empty level".into()));
        }
        let mut grid = Grid {
            size: [sx, sy, sz],
            cells: vec![None; sx * sy * sz],
        };
        for (y, layer) in level.layers.iter().enumerate() {
            if layer.len() != sz {
                return Err(ctx(format!(
                    "layer {y} has {} rows, expected {sz}",
                    layer.len()
                )));
            }
            for (z, row) in layer.iter().enumerate() {
                if row.chars().count() != sx {
                    return Err(ctx(format!("layer {y} row {z} width != {sx}")));
                }
                for (x, ch) in row.chars().enumerate() {
                    if ch == '.' {
                        continue;
                    }
                    let state = palette
                        .get(&ch)
                        .ok_or_else(|| ctx(format!("palette has no {ch:?}")))?;
                    let index = grid.index(x, y, z);
                    grid.cells[index] = Some(*state);
                }
            }
        }
        auto_connect(registry, &mut grid).map_err(ctx)?;
        let limit = MAX_SIZE;
        if sx > usize::from(limit[0]) || sy > usize::from(limit[1]) || sz > usize::from(limit[2]) {
            return Err(ctx(format!("size {sx}x{sy}x{sz} exceeds {limit:?}")));
        }
        let cert = level.cert.map(usize::from);
        if cert.iter().zip(grid.size).any(|(c, s)| *c == 0 || *c > s) {
            return Err(ctx("cert box must fit inside the level".into()));
        }
        if n > 0 {
            let prev: &Grid = &grids[n - 1];
            let prev_cert = raw.levels[n - 1].cert;
            if (0..3).any(|i| level.cert[i] < prev_cert[i] || grid.size[i] < prev.size[i]) {
                return Err(ctx("level must contain the previous level's box".into()));
            }
        }
        grids.push(grid);
    }
    let extent: [usize; 3] =
        std::array::from_fn(|i| grids.iter().map(|g| g.size[i]).max().unwrap_or(0));
    let (fx, fz) = (extent[0] as i32, extent[2] as i32);
    let mut levels = Vec::new();
    for (n, level) in raw.levels.iter().enumerate() {
        let grid = &grids[n];
        let prev = n.checked_sub(1).map(|p| &grids[p]);
        let mut placed: Vec<([i32; 3], BlockStateId)> = Vec::new();
        let mut removals: Vec<[i32; 3]> = Vec::new();
        let span: [usize; 3] =
            std::array::from_fn(|i| grid.size[i].max(prev.map_or(0, |p| p.size[i])));
        for y in 0..span[1] as i32 {
            for z in 0..span[2] as i32 {
                for x in 0..span[0] as i32 {
                    let (new, old) = (grid.at(x, y, z), prev.and_then(|p| p.at(x, y, z)));
                    match (new, old) {
                        (Some(n), o) if Some(n) != o => placed.push(([x, y, z], n)),
                        (None, Some(_)) => removals.push([x, y, z]),
                        _ => {}
                    }
                }
            }
        }
        placed.sort_by_key(|([x, y, z], id)| (*y, attachment(path_of(registry, *id)), *z, *x));
        let mut counts: BTreeMap<ItemId, u32> = BTreeMap::new();
        for (_, id) in &placed {
            if let Some((item, count)) =
                block_cost(registry, *id).map_err(|e| format!("{}: {e}", raw.id))?
            {
                *counts.entry(item).or_default() += count;
            }
        }
        let size = grid.size.map(|s| s as i32);
        let cert = level.cert.map(i32::from);
        let mut rotated = Vec::new();
        for turns in 0..4_usize {
            let map = |[x, y, z]: [i32; 3]| -> Result<[i8; 3], String> {
                let (rx, rz) = rotate_xz(turns, x, z, fx, fz);
                Ok([narrow(rx)?, narrow(y)?, narrow(rz)?])
            };
            let mut blocks = Vec::with_capacity(placed.len());
            for (at, id) in &placed {
                blocks.push((
                    map(*at)?,
                    rotate_state(registry, *id, turns).map_err(|e| format!("{}: {e}", raw.id))?,
                ));
            }
            let corner = |dx: i32, dz: i32, sx: i32, sz: i32| {
                rotate_xz(turns, dx * (sx - 1), dz * (sz - 1), fx, fz)
            };
            let span = |sx: i32, sz: i32| {
                let (a, b) = (corner(0, 0, sx, sz), corner(1, 1, sx, sz));
                ((a.0.min(b.0), a.1.min(b.1)), (a.0.max(b.0), a.1.max(b.1)))
            };
            let (extent_min, extent_max) = span(size[0], size[2]);
            let (cert_min, cert_max) = span(cert[0], cert[2]);
            rotated.push(RotatedLevel {
                blocks,
                removals: removals.iter().map(|r| map(*r)).collect::<Result<_, _>>()?,
                size: [
                    u8::try_from(extent_max.0 - extent_min.0 + 1).map_err(|_| "size")?,
                    grid.size[1] as u8,
                    u8::try_from(extent_max.1 - extent_min.1 + 1).map_err(|_| "size")?,
                ],
                entrance: map(level.entrance.map(i32::from))?,
                cert_min: [narrow(cert_min.0)?, 0, narrow(cert_min.1)?],
                cert_max: [
                    narrow(cert_max.0)?,
                    narrow(cert[1] - 1)?,
                    narrow(cert_max.1)?,
                ],
            });
        }
        levels.push(BlueprintLevel {
            size: grid.size.map(|s| s as u8),
            cert: level.cert,
            blocks: placed
                .iter()
                .map(|(at, id)| Ok((map_r0(*at)?, *id)))
                .collect::<Result<_, String>>()?,
            removals: removals
                .iter()
                .map(|r| map_r0(*r))
                .collect::<Result<_, _>>()?,
            materials: counts.into_iter().collect(),
            entrance: level.entrance,
            rotated: rotated.try_into().map_err(|_| "four rotations")?,
        });
    }
    Ok(Blueprint {
        id: raw.id,
        kind,
        levels,
        extent: extent.map(|e| e as u8),
    })
}
fn map_r0([x, y, z]: [i32; 3]) -> Result<[i8; 3], String> {
    Ok([narrow(x)?, narrow(y)?, narrow(z)?])
}

pub(crate) fn load(registry: &CoreRegistry) -> Result<Vec<Blueprint>, String> {
    SOURCES
        .iter()
        .map(|source| {
            let raw: RawBlueprint = serde_json::from_str(source).map_err(|e| e.to_string())?;
            load_blueprint(registry, raw)
        })
        .collect()
}

#[cfg(test)]
#[path = "blueprints_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "blueprints_function_tests.rs"]
mod function_tests;
