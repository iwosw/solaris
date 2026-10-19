//! Player-built certification. Scans happen only on explicit certify/upgrade;
//! allocator reads the persisted, bounded work-point list, never building volumes.
use std::collections::BTreeSet;
use std::fmt;

use mc_script::ScriptWorldBounds;
use serde::{Deserialize, Serialize};

use super::{Registry, SettlementId, SettlementRecord};
use crate::boundary::BlockPos;
use crate::tick::TickState;
use crate::villager::{VillagerProfession as Profession, VillagerState};

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BuildingId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BuildingKind {
    House,
    Storage,
    Workshop(Profession),
    Mine,
    LumberCamp,
    Barracks,
    Wall,
    Tower,
    Gate,
}
impl BuildingKind {
    #[must_use]
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "house" => Self::House,
            "storage" => Self::Storage,
            "mine" => Self::Mine,
            "lumber_camp" | "lumbercamp" => Self::LumberCamp,
            "barracks" => Self::Barracks,
            "wall" => Self::Wall,
            "tower" => Self::Tower,
            "gate" => Self::Gate,
            "armorer" => Self::Workshop(Profession::Armorer),
            "butcher" => Self::Workshop(Profession::Butcher),
            "cartographer" => Self::Workshop(Profession::Cartographer),
            "cleric" => Self::Workshop(Profession::Cleric),
            "farmer" => Self::Workshop(Profession::Farmer),
            "fisherman" => Self::Workshop(Profession::Fisherman),
            "fletcher" => Self::Workshop(Profession::Fletcher),
            "leatherworker" => Self::Workshop(Profession::Leatherworker),
            "librarian" => Self::Workshop(Profession::Librarian),
            "mason" => Self::Workshop(Profession::Mason),
            "shepherd" => Self::Workshop(Profession::Shepherd),
            "toolsmith" => Self::Workshop(Profession::Toolsmith),
            "weaponsmith" => Self::Workshop(Profession::Weaponsmith),
            _ => return None,
        })
    }
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Self::House => "house",
            Self::Storage => "storage",
            Self::Mine => "mine",
            Self::LumberCamp => "lumber_camp",
            Self::Barracks => "barracks",
            Self::Wall => "wall",
            Self::Tower => "tower",
            Self::Gate => "gate",
            Self::Workshop(p) => match p {
                Profession::Armorer => "armorer",
                Profession::Butcher => "butcher",
                Profession::Cartographer => "cartographer",
                Profession::Cleric => "cleric",
                Profession::Farmer => "farmer",
                Profession::Fisherman => "fisherman",
                Profession::Fletcher => "fletcher",
                Profession::Leatherworker => "leatherworker",
                Profession::Librarian => "librarian",
                Profession::Mason => "mason",
                Profession::Shepherd => "shepherd",
                Profession::Toolsmith => "toolsmith",
                Profession::Weaponsmith => "weaponsmith",
                _ => "invalid_workshop",
            },
        }
    }
    #[must_use]
    pub fn profession(self) -> Option<Profession> {
        match self {
            Self::Workshop(p)
                if !matches!(
                    p,
                    Profession::None
                        | Profession::Nitwit
                        | Profession::Miner
                        | Profession::Lumberjack
                ) =>
            {
                Some(p)
            }
            Self::Mine => Some(Profession::Miner),
            Self::LumberCamp => Some(Profession::Lumberjack),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Building {
    pub id: BuildingId,
    pub settlement: SettlementId,
    pub kind: BuildingKind,
    pub level: u8,
    pub bounds: ScriptWorldBounds,
    /// Sorted workstation positions (or container positions for Storage).
    pub work_points: Vec<[i32; 3]>,
    pub revision: u64,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Requirement {
    pub description: String,
    pub required: u32,
    pub found: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CertError {
    pub missing: Vec<Requirement>,
}
impl fmt::Display for CertError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, r) in self.missing.iter().enumerate() {
            if index != 0 {
                write!(f, "; ")?;
            }
            write!(f, "{}: {}/{}", r.description, r.found, r.required)?;
        }
        Ok(())
    }
}
impl std::error::Error for CertError {}
fn failure(description: &str) -> CertError {
    CertError {
        missing: vec![Requirement {
            description: description.into(),
            required: 1,
            found: 0,
        }],
    }
}
#[must_use]
pub fn job_slots(kind: BuildingKind, level: u8) -> usize {
    if kind.profession().is_some() && (1..=3).contains(&level) {
        usize::from(level)
    } else {
        0
    }
}
#[must_use]
pub fn requirements(kind: BuildingKind, level: u8) -> Vec<Requirement> {
    let l = u32::from(level.clamp(1, 3));
    let height = if kind == BuildingKind::Tower {
        3 * l + 3
    } else {
        4
    };
    let mut entries = vec![("footprint", (l + 2).pow(2)), ("height", height)];
    match kind {
        BuildingKind::House | BuildingKind::Barracks => {
            entries.extend([
                (
                    "beds",
                    if kind == BuildingKind::Barracks {
                        l * 2
                    } else {
                        l
                    },
                ),
                ("roof", 1),
                ("enclosure", 1),
                ("entrance", 1),
            ]);
        }
        BuildingKind::Storage => entries.push(("containers", l)),
        BuildingKind::Workshop(_) | BuildingKind::Mine | BuildingKind::LumberCamp => {
            entries.push(("workstations", l))
        }
        BuildingKind::Wall => {
            entries = vec![
                ("wall length", 4 * l + 1),
                ("wall height", l + 2),
                ("continuous wall", 1),
            ]
        }
        BuildingKind::Tower => entries.extend([("roof", 1), ("enclosure", 1)]),
        BuildingKind::Gate => entries.extend([("entrance", l), ("gate frame", 1)]),
    }
    entries
        .into_iter()
        .map(|(description, required)| Requirement {
            description: description.into(),
            required,
            found: 0,
        })
        .collect()
}
fn dimensions(bounds: ScriptWorldBounds) -> Option<[u32; 3]> {
    let mut dims = [0; 3];
    for (axis, dim) in dims.iter_mut().enumerate() {
        *dim = u32::try_from(i64::from(bounds.max[axis]) - i64::from(bounds.min[axis]) + 1).ok()?;
        if *dim == 0 || *dim > 64 {
            return None;
        }
    }
    (u64::from(dims[0]) * u64::from(dims[1]) * u64::from(dims[2]) <= 32768).then_some(dims)
}
fn inside(bounds: ScriptWorldBounds, pos: [i32; 3]) -> bool {
    (0..3).all(|i| pos[i] >= bounds.min[i] && pos[i] <= bounds.max[i])
}
fn overlaps(a: ScriptWorldBounds, b: ScriptWorldBounds) -> bool {
    (0..3).all(|i| a.min[i] <= b.max[i] && b.min[i] <= a.max[i])
}
fn within(record: &SettlementRecord, bounds: ScriptWorldBounds) -> bool {
    [bounds.min[0], bounds.max[0]].into_iter().all(|x| {
        [bounds.min[1], bounds.max[1]].into_iter().all(|y| {
            [bounds.min[2], bounds.max[2]].into_iter().all(|z| {
                [x, y, z]
                    .into_iter()
                    .zip(record.anchor)
                    .map(|(v, a)| (i128::from(v) - i128::from(a)).pow(2))
                    .sum::<i128>()
                    <= i128::from(record.radius).pow(2)
            })
        })
    })
}
fn expected_station(kind: BuildingKind, p: Profession) -> bool {
    match kind {
        BuildingKind::Mine => p == Profession::Toolsmith,
        BuildingKind::LumberCamp => p == Profession::Fletcher,
        BuildingKind::Workshop(expected) => expected == p,
        _ => false,
    }
}
#[allow(clippy::too_many_lines)]
pub(crate) fn inspect(
    state: &TickState,
    kind: BuildingKind,
    level: u8,
    bounds: ScriptWorldBounds,
) -> Result<Vec<[i32; 3]>, CertError> {
    let dims = dimensions(bounds).ok_or_else(|| {
        failure("bounds must be ordered, at most 64 blocks per axis and 32768 blocks total")
    })?;
    if matches!(kind, BuildingKind::Workshop(_)) && kind.profession().is_none() {
        return Err(failure("vanilla workshop profession required"));
    }
    let mut beds = 0;
    let mut entrances = 0;
    let mut points = BTreeSet::new();
    let mut roof = true;
    let mut enclosure = true;
    let mut floor = true;
    let mut wall = true;
    let mut gate_frame = true;
    for x in bounds.min[0]..=bounds.max[0] {
        for z in bounds.min[2]..=bounds.max[2] {
            for y in bounds.min[1]..=bounds.max[1] {
                let id = state
                    .world
                    .block_at(x, y, z)
                    .ok_or_else(|| failure("all building chunks must be loaded"))?;
                let definition = state
                    .registry
                    .blocks
                    .by_id(mc_world::BlockStateId(id))
                    .ok_or_else(|| failure("unknown block"))?;
                let name = definition.block.id.path();
                let prop = |key: &str, value: &str| {
                    definition
                        .properties
                        .iter()
                        .any(|(k, v)| k == key && v == value)
                };
                let solid = !state.registry.collision_boxes[id as usize].is_empty();
                let edge = x == bounds.min[0]
                    || x == bounds.max[0]
                    || z == bounds.min[2]
                    || z == bounds.max[2];
                if y == bounds.max[1] {
                    roof &= solid;
                }
                if y == bounds.min[1] {
                    floor &= solid;
                }
                if edge && y > bounds.min[1] && y < bounds.max[1] {
                    enclosure &= solid;
                }
                if (if dims[0] >= dims[2] {
                    z == bounds.min[2]
                } else {
                    x == bounds.min[0]
                }) && y < bounds.min[1] + i32::from(level) + 2
                {
                    wall &= solid;
                }
                if (x == bounds.min[0] || x == bounds.max[0])
                    && (z == bounds.min[2] || z == bounds.max[2])
                {
                    gate_frame &= solid;
                }
                if name.ends_with("_bed")
                    && prop("part", "head")
                    && super::allocator::bed_has_baby_space(state, BlockPos { x, y, z })
                {
                    beds += 1;
                }
                if name.ends_with("_door") && prop("half", "lower") {
                    entrances += 1;
                }
                if kind == BuildingKind::Storage
                    && matches!(name, "chest" | "trapped_chest" | "barrel")
                {
                    points.insert([x, y, z]);
                }
                if let Some(super::allocator::PoiKind::Job(p)) =
                    super::allocator::poi_kind(&state.registry, id)
                    && expected_station(kind, p)
                {
                    points.insert([x, y, z]);
                }
            }
        }
    }
    let missing: Vec<_> = requirements(kind, level)
        .into_iter()
        .filter_map(|mut r| {
            r.found = match r.description.as_str() {
                "footprint" => {
                    if floor || kind == BuildingKind::Mine {
                        dims[0] * dims[2]
                    } else {
                        0
                    }
                }
                "height" => dims[1],
                "beds" => beds,
                "containers" | "workstations" => points.len() as u32,
                "roof" => u32::from(roof),
                "enclosure" => u32::from(enclosure),
                "entrance" => entrances,
                "wall length" => dims[0].max(dims[2]),
                "wall height" => dims[1],
                "continuous wall" => u32::from(wall),
                "gate frame" => u32::from(gate_frame),
                _ => 0,
            };
            (r.found < r.required).then_some(r)
        })
        .collect();
    if !missing.is_empty() {
        return Err(CertError { missing });
    }
    let cap = if kind == BuildingKind::Storage {
        64
    } else {
        job_slots(kind, level)
    };
    Ok(points.into_iter().take(cap).collect())
}

pub(crate) fn certify(
    state: &mut TickState,
    settlement: SettlementId,
    kind: BuildingKind,
    bounds: ScriptWorldBounds,
) -> Result<BuildingId, CertError> {
    let record = state
        .settlements
        .get(settlement)
        .ok_or_else(|| failure("settlement not found"))?;
    if dimensions(bounds).is_none() || !within(record, bounds) {
        return Err(failure("invalid bounds or outside settlement"));
    }
    let mut overlapping = record
        .buildings
        .values()
        .filter(|b| overlaps(b.bounds, bounds));
    if let Some(existing) = overlapping.next() {
        if overlapping.next().is_some()
            || existing.kind != kind
            || !inside(bounds, existing.bounds.min)
            || !inside(bounds, existing.bounds.max)
        {
            return Err(failure(
                "building overlaps another certificate; expansion must contain the original bounds",
            ));
        }
        let (id, level) = (existing.id, existing.level);
        let points = inspect(state, kind, level, bounds)?;
        let record = state
            .settlements
            .get_mut(settlement)
            .expect("validated settlement");
        record.revision = record.revision.saturating_add(1);
        let building = record.buildings.get_mut(&id).expect("validated building");
        building.bounds = bounds;
        building.work_points = points;
        building.revision = building.revision.saturating_add(1);
        wake_residents(state, settlement);
        return Ok(id);
    }
    if record.buildings.len() >= 1024 {
        return Err(failure("maximum 1024 buildings per settlement"));
    }
    let sequence = record
        .building_sequence
        .checked_add(1)
        .filter(|n| *n <= u64::from(u32::MAX))
        .ok_or_else(|| failure("building identity exhausted"))?;
    if settlement.0 > u64::from(u32::MAX) {
        return Err(failure("settlement identity exhausted"));
    }
    let points = inspect(state, kind, 1, bounds)?;
    let id = BuildingId((settlement.0 << 32) | sequence);
    let record = state
        .settlements
        .get_mut(settlement)
        .expect("validated settlement");
    record.building_sequence = sequence;
    record.revision = record.revision.saturating_add(1);
    record.buildings.insert(
        id,
        Building {
            id,
            settlement,
            kind,
            level: 1,
            bounds,
            work_points: points,
            revision: 1,
        },
    );
    wake_residents(state, settlement);
    Ok(id)
}
pub(crate) fn upgrade(state: &mut TickState, id: BuildingId) -> Result<(), CertError> {
    let settlement = SettlementId(id.0 >> 32);
    let record = state
        .settlements
        .get(settlement)
        .ok_or_else(|| failure("settlement not found"))?;
    let building = record
        .buildings
        .get(&id)
        .ok_or_else(|| failure("building not found"))?;
    if building.level >= 3 {
        return Err(failure("maximum level is 3"));
    }
    let points = inspect(state, building.kind, building.level + 1, building.bounds)?;
    let record = state
        .settlements
        .get_mut(settlement)
        .expect("validated settlement");
    record.revision = record.revision.saturating_add(1);
    let building = record.buildings.get_mut(&id).expect("validated building");
    building.level += 1;
    building.revision = building.revision.saturating_add(1);
    building.work_points = points;
    wake_residents(state, settlement);
    Ok(())
}

fn wake_residents(state: &mut TickState, settlement: SettlementId) {
    for slot in state.entities.slots() {
        if state
            .entities
            .row(slot)
            .villager
            .as_ref()
            .is_some_and(|v| v.settlement == Some(settlement))
        {
            let id = state.entities.body(slot).id;
            state.villager_pois.search_at.remove(&id);
            state.villager_pois.dirty_claim(id);
        }
    }
}
/// Certification is the job authority; the allocator separately validates the live workstation.
pub(crate) fn job_profession(
    registry: &Registry,
    settlement: Option<SettlementId>,
    pos: BlockPos,
) -> Option<Profession> {
    let record = registry.get(settlement?)?;
    record.buildings.values().find_map(|b| {
        b.work_points
            .iter()
            .take(job_slots(b.kind, b.level))
            .any(|p| *p == [pos.x, pos.y, pos.z])
            .then(|| b.kind.profession())
            .flatten()
    })
}
pub(crate) fn building_for_job<'a>(
    state: &'a TickState,
    v: &VillagerState,
) -> Option<&'a Building> {
    if v.job_site_dimension != state.dimension.name() {
        return None;
    }
    let job = v.job_site?;
    state
        .settlements
        .get(v.settlement?)?
        .buildings
        .values()
        .find(|b| {
            b.kind.profession().is_some()
                && b.work_points
                    .iter()
                    .take(job_slots(b.kind, b.level))
                    .any(|p| *p == [job.x, job.y, job.z])
        })
}
pub(crate) fn storage_points(state: &TickState, settlement: SettlementId) -> Vec<[i32; 3]> {
    state
        .settlements
        .get(settlement)
        .into_iter()
        .flat_map(|r| r.buildings.values())
        .filter(|b| b.kind == BuildingKind::Storage)
        .flat_map(|b| b.work_points.iter().copied())
        .collect()
}
pub(super) fn validate_record_buildings(record: &SettlementRecord) -> Result<(), &'static str> {
    if record.building_sequence > u64::from(u32::MAX) || record.buildings.len() > 1024 {
        return Err("building capacity exceeded");
    }
    for (id, b) in &record.buildings {
        if b.id != *id
            || b.settlement != record.id
            || id.0 >> 32 != record.id.0
            || id.0 & u64::from(u32::MAX) == 0
            || id.0 & u64::from(u32::MAX) > record.building_sequence
            || !(1..=3).contains(&b.level)
            || b.revision == 0
            || dimensions(b.bounds).is_none()
            || !within(record, b.bounds)
        {
            return Err("invalid certified building");
        }
        let cap = if b.kind == BuildingKind::Storage {
            64
        } else {
            job_slots(b.kind, b.level)
        };
        if b.work_points.len() > cap
            || b.work_points.windows(2).any(|p| p[0] >= p[1])
            || b.work_points.iter().any(|p| !inside(b.bounds, *p))
            || (b.kind.profession().is_some() && b.work_points.len() != cap)
            || (b.kind == BuildingKind::Storage && b.work_points.len() < usize::from(b.level))
            || (matches!(b.kind, BuildingKind::Workshop(_)) && b.kind.profession().is_none())
        {
            return Err("invalid certified building work points");
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "buildings_tests.rs"]
mod tests;
