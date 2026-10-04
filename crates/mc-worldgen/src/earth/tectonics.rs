//! Finite-plane tectonics. All distances are physical metres; time is synthetic Ma.
//! Height, thermal subsidence and geological attenuation below are empirical proxies,
//! not a solution of mantle flow or isostasy. No independent elevation noise exists.
use super::grid::Grid;
use super::prepared::PrepareError;
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeMap, BinaryHeap, VecDeque};

const PLATE_SPACING_M: f64 = 5_000_000.0;
const MOTION_EPSILON_M_PER_MA: f64 = 1_000.0;

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Point {
    pub x_m: f64,
    pub z_m: f64,
}
impl Point {
    fn sub(self, b: Self) -> Self {
        Self {
            x_m: self.x_m - b.x_m,
            z_m: self.z_m - b.z_m,
        }
    }
    fn add(self, b: Self) -> Self {
        Self {
            x_m: self.x_m + b.x_m,
            z_m: self.z_m + b.z_m,
        }
    }
    fn scale(self, a: f64) -> Self {
        Self {
            x_m: self.x_m * a,
            z_m: self.z_m * a,
        }
    }
    fn dot(self, b: Self) -> f64 {
        self.x_m * b.x_m + self.z_m * b.z_m
    }
    fn norm(self) -> f64 {
        self.dot(self).sqrt()
    }
    fn unit(self) -> Self {
        self.scale(1.0 / self.norm())
    }
    fn finite(self) -> bool {
        self.x_m.is_finite() && self.z_m.is_finite()
    }
}

fn point_order(a: Point, b: Point) -> Ordering {
    a.x_m
        .total_cmp(&b.x_m)
        .then_with(|| a.z_m.total_cmp(&b.z_m))
}
fn point(grid: &Grid, i: usize) -> Point {
    let (x_m, z_m) = grid.center(i);
    Point { x_m, z_m }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VelocityEpoch {
    /// Start age before present; epochs sorted youngest first, starting at zero.
    pub start_age_ma: f64,
    pub velocity_m_per_ma: Point,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Plate {
    pub id: usize,
    pub nucleus: Point,
    pub growth_weight: f64,
    pub velocity_history: Vec<VelocityEpoch>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Footprint {
    pub center: Point,
    pub radius_x_m: f64,
    pub radius_z_m: f64,
    pub angle_rad: f64,
}
impl Footprint {
    fn radius(&self, p: Point) -> f64 {
        let d = p.sub(self.center);
        let (s, c) = self.angle_rad.sin_cos();
        ((d.x_m * c + d.z_m * s).powi(2) / self.radius_x_m.powi(2)
            + (-d.x_m * s + d.z_m * c).powi(2) / self.radius_z_m.powi(2))
        .sqrt()
    }
    fn validate(&self) -> bool {
        self.center.finite()
            && self.radius_x_m.is_finite()
            && self.radius_x_m > 0.0
            && self.radius_z_m.is_finite()
            && self.radius_z_m > 0.0
            && self.angle_rad.is_finite()
    }
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrustFragment {
    pub id: usize,
    pub footprint: Footprint,
    pub thickness_m: f64,
    pub basement_age_ma: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CrustCell {
    pub continental_fraction: f64,
    pub thickness_m: f64,
    pub basement_age_ma: f64,
    pub ocean_floor_age_ma: f64,
    pub lithology_id: usize,
    pub sediment_cover_m: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ProvinceKind {
    AncientOrogen,
    ExtinctArc,
    OldRift,
    PassiveMargin,
    SedimentaryBasin,
    Craton,
    ColdPhase,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Province {
    pub id: usize,
    pub kind: ProvinceKind,
    pub footprint: Footprint,
    pub age_ma: f64,
    pub initial_relief_m: f64,
    pub basement_lithology_id: usize,
    pub sediment_cover_m: f64,
    pub folding: f64,
    pub intrusion_fraction: f64,
    pub mineralization: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Lithology {
    pub id: usize,
    pub name: String,
    pub density_kg_m3: f64,
    pub erodibility: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum BoundaryRegime {
    Collision,
    Subduction,
    OceanicSpreading,
    ContinentalRift,
    Transform,
    Passive,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoundarySegment {
    pub id: usize,
    pub plate_a: usize,
    pub plate_b: usize,
    /// Shared polyline oriented so its right-hand normal points from A to B.
    pub points: Vec<Point>,
    pub relative_velocity_m_per_ma: Point,
    pub normal_motion_m_per_ma: f64,
    pub shear_m_per_ma: f64,
    pub regime: BoundaryRegime,
    /// Fixed for the entire connected segment, never independently selected per cell.
    pub subducting_plate: Option<usize>,
    /// Synthetic overriding-plate retreat proxy; not every arc has a basin.
    pub backarc_extension: bool,
    pub duration_ma: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Structure {
    pub boundary_id: usize,
    pub basement_lithology_id: usize,
    pub folding: f64,
    pub fault_displacement_m: f64,
    pub intrusion_fraction: f64,
    pub mineralization: f64,
    /// Offsets signed toward the overriding plate; zero for non-subduction.
    pub trench_offset_m: f64,
    pub forearc_offset_m: f64,
    pub arc_offset_m: f64,
    pub backarc_offset_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Hotspot {
    pub id: usize,
    pub plate_id: usize,
    pub source: Point,
    pub duration_ma: f64,
    pub interval_ma: f64,
    pub radius_m: f64,
    pub initial_height_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Volcano {
    pub hotspot_id: usize,
    pub plate_id: usize,
    pub center: Point,
    pub age_ma: f64,
    pub radius_m: f64,
    pub height_m: f64,
    pub subsidence_m: f64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TectonicPlan {
    pub elevation_m: Vec<f64>,
    pub erodibility: Vec<f64>,
    pub oceanic: Vec<bool>,
    pub cold_phase: Vec<bool>,
    pub plate_id: Vec<usize>,
    pub crust: Vec<CrustCell>,
    pub plates: Vec<Plate>,
    pub fragments: Vec<CrustFragment>,
    pub boundaries: Vec<BoundarySegment>,
    pub provinces: Vec<Province>,
    pub lithologies: Vec<Lithology>,
    pub structures: Vec<Structure>,
    pub hotspots: Vec<Hotspot>,
    pub volcanoes: Vec<Volcano>,
}

fn invalid(message: &'static str) -> PrepareError {
    PrepareError::Invalid(message)
}
fn nonnegative(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}
fn fraction(v: f64) -> bool {
    v.is_finite() && (0.0..=1.0).contains(&v)
}
fn unique(ids: impl Iterator<Item = usize>) -> bool {
    let mut set = std::collections::BTreeSet::new();
    ids.into_iter().all(|id| set.insert(id))
}
impl TectonicPlan {
    pub fn validate(&self, grid: &Grid) -> Result<(), PrepareError> {
        grid.validate()?;
        let n = grid.len();
        if [
            self.elevation_m.len(),
            self.erodibility.len(),
            self.oceanic.len(),
            self.cold_phase.len(),
            self.plate_id.len(),
            self.crust.len(),
        ]
        .into_iter()
        .any(|l| l != n)
            || self.elevation_m.iter().any(|v| !v.is_finite())
            || self.erodibility.iter().any(|v| !fraction(*v))
        {
            return Err(invalid("invalid tectonic raster"));
        }
        if self.plates.is_empty()
            || !unique(self.plates.iter().map(|p| p.id))
            || !unique(self.fragments.iter().map(|p| p.id))
            || !unique(self.provinces.iter().map(|p| p.id))
            || !unique(self.boundaries.iter().map(|p| p.id))
            || !unique(self.hotspots.iter().map(|p| p.id))
            || !unique(self.lithologies.iter().map(|p| p.id))
        {
            return Err(invalid("duplicate tectonic catalog IDs"));
        }
        let known_plate = |id| self.plates.iter().any(|p| p.id == id);
        let known_rock = |id| self.lithologies.iter().any(|p| p.id == id);
        for p in &self.plates {
            if !p.nucleus.finite()
                || !p.growth_weight.is_finite()
                || p.growth_weight <= 0.0
                || p.velocity_history.is_empty()
                || p.velocity_history[0].start_age_ma != 0.0
                || p.velocity_history
                    .iter()
                    .any(|e| !nonnegative(e.start_age_ma) || !e.velocity_m_per_ma.finite())
                || p.velocity_history
                    .windows(2)
                    .any(|e| e[0].start_age_ma >= e[1].start_age_ma)
                || !self.plate_id.contains(&p.id)
            {
                return Err(invalid("invalid plate kinematics"));
            }
        }
        let mut visited = vec![false; n];
        for start in 0..n {
            if visited[start] {
                continue;
            }
            let id = self.plate_id[start];
            if !known_plate(id) {
                return Err(invalid("unknown raster plate"));
            }
            let mut queue = VecDeque::from([start]);
            visited[start] = true;
            while let Some(i) = queue.pop_front() {
                for j in grid.neighbours(i) {
                    if !visited[j] && self.plate_id[j] == id {
                        visited[j] = true;
                        queue.push_back(j);
                    }
                }
            }
            if (start + 1..n).any(|i| self.plate_id[i] == id && !visited[i]) {
                return Err(invalid("disconnected plate"));
            }
        }
        for (i, c) in self.crust.iter().enumerate() {
            if !fraction(c.continental_fraction)
                || !nonnegative(c.thickness_m)
                || c.thickness_m == 0.0
                || !nonnegative(c.basement_age_ma)
                || !nonnegative(c.ocean_floor_age_ma)
                || !known_rock(c.lithology_id)
                || !nonnegative(c.sediment_cover_m)
                || self.oceanic[i] != (c.continental_fraction < 0.5)
            {
                return Err(invalid("invalid crust cell"));
            }
        }
        for f in &self.fragments {
            if !f.footprint.validate()
                || !nonnegative(f.thickness_m)
                || f.thickness_m == 0.0
                || !nonnegative(f.basement_age_ma)
            {
                return Err(invalid("invalid crust fragment"));
            }
        }
        for p in &self.provinces {
            if !p.footprint.validate()
                || !nonnegative(p.age_ma)
                || !p.initial_relief_m.is_finite()
                || !known_rock(p.basement_lithology_id)
                || !nonnegative(p.sediment_cover_m)
                || !fraction(p.folding)
                || !fraction(p.intrusion_fraction)
                || !fraction(p.mineralization)
            {
                return Err(invalid("invalid inherited province"));
            }
        }
        for b in &self.boundaries {
            if !known_plate(b.plate_a)
                || !known_plate(b.plate_b)
                || b.plate_a == b.plate_b
                || b.points.len() < 2
                || b.points.iter().any(|p| !p.finite())
                || b.points.windows(2).any(|p| p[0] == p[1])
                || !b.relative_velocity_m_per_ma.finite()
                || !b.normal_motion_m_per_ma.is_finite()
                || !b.shear_m_per_ma.is_finite()
                || !nonnegative(b.duration_ma)
                || b.subducting_plate
                    .is_some_and(|id| id != b.plate_a && id != b.plate_b)
                || (b.regime == BoundaryRegime::Subduction) != b.subducting_plate.is_some()
                || (b.backarc_extension && b.regime != BoundaryRegime::Subduction)
            {
                return Err(invalid("invalid canonical boundary"));
            }
        }
        if self.structures.len() != self.boundaries.len()
            || !unique(self.structures.iter().map(|s| s.boundary_id))
        {
            return Err(invalid("unbalanced structure catalog"));
        }
        for s in &self.structures {
            if !self.boundaries.iter().any(|b| b.id == s.boundary_id)
                || !known_rock(s.basement_lithology_id)
                || !fraction(s.folding)
                || !fraction(s.intrusion_fraction)
                || !fraction(s.mineralization)
                || !s.fault_displacement_m.is_finite()
                || ![
                    s.trench_offset_m,
                    s.forearc_offset_m,
                    s.arc_offset_m,
                    s.backarc_offset_m,
                ]
                .into_iter()
                .all(f64::is_finite)
            {
                return Err(invalid("invalid structural provenance"));
            }
        }
        for l in &self.lithologies {
            if l.name.is_empty()
                || !l.density_kg_m3.is_finite()
                || l.density_kg_m3 <= 0.0
                || !fraction(l.erodibility)
            {
                return Err(invalid("invalid lithology"));
            }
        }
        for h in &self.hotspots {
            if !known_plate(h.plate_id)
                || !h.source.finite()
                || !nonnegative(h.duration_ma)
                || !h.interval_ma.is_finite()
                || h.interval_ma <= 0.0
                || !h.radius_m.is_finite()
                || h.radius_m <= 0.0
                || !nonnegative(h.initial_height_m)
            {
                return Err(invalid("invalid hotspot"));
            }
            let intervals = (h.duration_ma / h.interval_ma).floor();
            let mut objects = 0;
            for v in self.volcanoes.iter().filter(|v| v.hotspot_id == h.id) {
                if (v.age_ma - objects as f64 * h.interval_ma).abs() > 1e-9 {
                    return Err(invalid("unordered hotspot chain chronology"));
                }
                objects += 1;
            }
            if !intervals.is_finite()
                || intervals >= usize::MAX as f64
                || objects != intervals as usize + 1
            {
                return Err(invalid("unbalanced hotspot chain chronology"));
            }
        }
        for v in &self.volcanoes {
            if !self
                .hotspots
                .iter()
                .any(|h| h.id == v.hotspot_id && h.plate_id == v.plate_id)
                || !v.center.finite()
                || !nonnegative(v.age_ma)
                || !v.radius_m.is_finite()
                || v.radius_m <= 0.0
                || !nonnegative(v.height_m)
                || !nonnegative(v.subsidence_m)
            {
                return Err(invalid("invalid hotspot chain"));
            }
        }
        Ok(())
    }
}

struct Random(u64);
impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e3779b97f4a7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58476d1ce4e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d049bb133111eb);
        z ^ (z >> 31)
    }
    fn unit(&mut self) -> f64 {
        (self.next() >> 11) as f64 / ((1u64 << 53) as f64)
    }
}
#[derive(Clone, Copy)]
struct Front {
    cost: f64,
    cell: usize,
    plate: usize,
}
impl PartialEq for Front {
    fn eq(&self, b: &Self) -> bool {
        self.cost == b.cost && self.cell == b.cell && self.plate == b.plate
    }
}
impl Eq for Front {}
impl Ord for Front {
    fn cmp(&self, b: &Self) -> Ordering {
        b.cost
            .total_cmp(&self.cost)
            .then_with(|| b.plate.cmp(&self.plate))
            .then_with(|| b.cell.cmp(&self.cell))
    }
}
impl PartialOrd for Front {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}

/// Connected weighted growth on the physical grid. Every accepted non-seed cell
/// has an accepted same-plate predecessor: connectivity is constructive, not repaired.
fn partition(grid: &Grid, plates: &[Plate]) -> Vec<usize> {
    let mut ids = vec![usize::MAX; grid.len()];
    let mut heap = BinaryHeap::new();
    let mut order: Vec<usize> = (0..plates.len()).collect();
    order.sort_by(|a, b| {
        plates[*a]
            .nucleus
            .x_m
            .total_cmp(&plates[*b].nucleus.x_m)
            .then_with(|| plates[*a].nucleus.z_m.total_cmp(&plates[*b].nucleus.z_m))
    });
    let metric: Vec<(f64, f64)> = order
        .iter()
        .map(|i| {
            let plate = &plates[*i];
            let anisotropy = (plate.nucleus.x_m / PLATE_SPACING_M
                + 1.618 * plate.nucleus.z_m / PLATE_SPACING_M)
                .sin();
            (
                (1.0 + 0.18 * anisotropy) / plate.growth_weight,
                1.0 / plate.growth_weight,
            )
        })
        .collect();
    // Geometry-owned bounded deformation: 8% metric variation at wavelengths
    // 0.8/1.2 Mm, shared by all fronts. It never contributes height directly.
    let anchor = plates[order[0]].nucleus;
    let warp: Vec<f64> = (0..grid.len())
        .map(|i| {
            let d = point(grid, i).sub(anchor);
            1.0 + 0.08 * (d.x_m / 800_000.0).sin() * (d.z_m / 1_200_000.0).sin()
        })
        .collect();
    for (p, index) in order.iter().enumerate() {
        let plate = &plates[*index];
        heap.push(Front {
            cost: 0.0,
            cell: grid.index_at(plate.nucleus.x_m, plate.nucleus.z_m),
            plate: p,
        });
    }
    while let Some(f) = heap.pop() {
        if ids[f.cell] != usize::MAX {
            continue;
        }
        ids[f.cell] = plates[order[f.plate]].id;
        for cell in grid.neighbours(f.cell) {
            if ids[cell] == usize::MAX {
                let d = point(grid, cell).sub(point(grid, f.cell));
                // Anisotropic but translation-invariant positive metric, plate-owned.
                let cost = (d.x_m.abs() * metric[f.plate].0 + d.z_m.abs() * metric[f.plate].1)
                    * 0.5
                    * (warp[cell] + warp[f.cell]);
                heap.push(Front {
                    cost: f.cost + cost,
                    cell,
                    plate: f.plate,
                });
            }
        }
    }
    ids
}
fn lithologies() -> Vec<Lithology> {
    [
        ("basalt", 2950.0, 0.3),
        ("granite", 2700.0, 0.15),
        ("metamorphic_fold_belt", 2800.0, 0.22),
        ("unconsolidated_sediment", 2200.0, 0.85),
        ("rift_volcanics", 2850.0, 0.4),
        ("arc_intrusive", 2750.0, 0.18),
    ]
    .into_iter()
    .enumerate()
    .map(|(id, (name, density_kg_m3, erodibility))| Lithology {
        id,
        name: name.into(),
        density_kg_m3,
        erodibility,
    })
    .collect()
}

pub fn build(grid: &Grid, seed: i64) -> Result<TectonicPlan, PrepareError> {
    grid.validate()?;
    if !(grid.width_m * grid.depth_m).is_finite()
        || !(grid.min_x_m + grid.width_m).is_finite()
        || !(grid.min_z_m + grid.depth_m).is_finite()
    {
        return Err(invalid("tectonic physical extent overflow"));
    }
    let mut rng = Random(seed as u64);
    // Sampling the fixed physical spacing, not shrinking plates to fit a tiny map.
    let nx = (grid.width_m / PLATE_SPACING_M).ceil().max(1.0) as usize;
    let nz = (grid.depth_m / PLATE_SPACING_M).ceil().max(1.0) as usize;
    if nx.checked_mul(nz).is_none_or(|count| count > grid.len()) {
        return Err(invalid("plate sampling density exceeds root precision"));
    }
    let mut plates = Vec::new();
    let mut nuclei = std::collections::BTreeSet::new();
    for z in 0..nz {
        for x in 0..nx {
            let nucleus = Point {
                x_m: grid.min_x_m
                    + ((x as f64 + 0.25 + 0.5 * rng.unit()) * PLATE_SPACING_M)
                        .min(grid.width_m * 0.95),
                z_m: grid.min_z_m
                    + ((z as f64 + 0.25 + 0.5 * rng.unit()) * PLATE_SPACING_M)
                        .min(grid.depth_m * 0.95),
            };
            let growth_weight = 0.7 + 0.65 * rng.unit();
            let v = Point {
                x_m: (rng.unit() - 0.5) * 100_000.0,
                z_m: (rng.unit() - 0.5) * 100_000.0,
            };
            let old = Point {
                x_m: (rng.unit() - 0.5) * 100_000.0,
                z_m: (rng.unit() - 0.5) * 100_000.0,
            };
            if nuclei.insert(grid.index_at(nucleus.x_m, nucleus.z_m)) {
                plates.push(Plate {
                    id: plates.len(),
                    nucleus,
                    growth_weight,
                    velocity_history: vec![
                        VelocityEpoch {
                            start_age_ma: 0.0,
                            velocity_m_per_ma: v,
                        },
                        VelocityEpoch {
                            start_age_ma: 20.0,
                            velocity_m_per_ma: old,
                        },
                    ],
                });
            }
        }
    }
    if grid.width_m.min(grid.depth_m) > 2_000_000.0 {
        let major_count = plates.len();
        for i in 0..major_count {
            if rng.unit() >= 0.18 {
                continue;
            }
            let center = plates[i].nucleus;
            let nucleus = Point {
                x_m: (center.x_m + (rng.unit() - 0.5) * 1_500_000.0)
                    .clamp(grid.min_x_m, grid.min_x_m + grid.width_m * 0.999),
                z_m: (center.z_m + (rng.unit() - 0.5) * 1_500_000.0)
                    .clamp(grid.min_z_m, grid.min_z_m + grid.depth_m * 0.999),
            };
            if !nuclei.insert(grid.index_at(nucleus.x_m, nucleus.z_m)) {
                continue;
            }
            plates.push(Plate {
                id: plates.len(),
                nucleus,
                growth_weight: 0.3 + 0.25 * rng.unit(),
                velocity_history: vec![VelocityEpoch {
                    start_age_ma: 0.0,
                    velocity_m_per_ma: Point {
                        x_m: (rng.unit() - 0.5) * 100_000.0,
                        z_m: (rng.unit() - 0.5) * 100_000.0,
                    },
                }],
            });
        }
    }
    let plate_id = partition(grid, &plates);
    // A tiny domain observes a subset of kilometre-scale crust/provinces.
    // Feature support may extend outside the physical integration grid; it adds
    // no cells or hydrological area, and never rescales a continent to the window.
    let support_width_m = grid.width_m.max(PLATE_SPACING_M);
    let support_depth_m = grid.depth_m.max(PLATE_SPACING_M);
    let support_min_x_m = grid.min_x_m - (support_width_m - grid.width_m) * 0.5;
    let support_min_z_m = grid.min_z_m - (support_depth_m - grid.depth_m) * 0.5;
    let count = ((grid.width_m * grid.depth_m) / (PLATE_SPACING_M * PLATE_SPACING_M) * 0.65)
        .ceil()
        .max(2.0) as usize;
    if count > grid.len().max(2) {
        return Err(invalid("crust sampling density exceeds root precision"));
    }
    let mut fragments = Vec::new();
    for id in 0..count {
        fragments.push(CrustFragment {
            id,
            footprint: Footprint {
                center: Point {
                    x_m: support_min_x_m + support_width_m * rng.unit(),
                    z_m: support_min_z_m + support_depth_m * rng.unit(),
                },
                radius_x_m: (0.12 + 0.22 * rng.unit()) * 7_000_000.0,
                radius_z_m: (0.12 + 0.22 * rng.unit()) * 7_000_000.0,
                angle_rad: rng.unit() * std::f64::consts::TAU,
            },
            thickness_m: 32_000.0 + 10_000.0 * rng.unit(),
            basement_age_ma: 500.0 + 2500.0 * rng.unit(),
        });
    }
    let mut crust = Vec::with_capacity(grid.len());
    for i in 0..grid.len() {
        let p = point(grid, i);
        let mut continental_fraction = 0.0_f64;
        let mut thick = 35_000.0;
        let mut age = 1500.0;
        for f in &fragments {
            let r = f.footprint.radius(p);
            let c = ((1.2 - r) / 0.4).clamp(0.0, 1.0);
            if c > continental_fraction {
                continental_fraction = c;
                thick = f.thickness_m;
                age = f.basement_age_ma;
            }
        }
        crust.push(CrustCell {
            continental_fraction,
            thickness_m: 7000.0 + (thick - 7000.0) * continental_fraction,
            basement_age_ma: age * continental_fraction + 120.0 * (1.0 - continental_fraction),
            ocean_floor_age_ma: 120.0,
            lithology_id: if continental_fraction >= 0.5 { 1 } else { 0 },
            sediment_cover_m: 0.0,
        });
    }
    let mut provinces = Vec::new();
    let kinds = [
        ProvinceKind::AncientOrogen,
        ProvinceKind::ExtinctArc,
        ProvinceKind::OldRift,
        ProvinceKind::PassiveMargin,
        ProvinceKind::SedimentaryBasin,
        ProvinceKind::Craton,
        ProvinceKind::ColdPhase,
    ];
    for (id, kind) in kinds.into_iter().enumerate() {
        let f = &fragments[id % fragments.len()];
        let mut footprint = f.footprint.clone();
        footprint.center.x_m += (rng.unit() - 0.5) * footprint.radius_x_m;
        footprint.center.z_m += (rng.unit() - 0.5) * footprint.radius_z_m;
        footprint.radius_x_m *= 0.8;
        footprint.radius_z_m *= if kind == ProvinceKind::AncientOrogen {
            0.25
        } else {
            0.7
        };
        let (relief, rock, sediment, fold, intrusion) = match kind {
            ProvinceKind::AncientOrogen => (2800.0, 2, 100.0, 0.9, 0.25),
            ProvinceKind::ExtinctArc => (1700.0, 5, 150.0, 0.3, 0.7),
            ProvinceKind::OldRift => (-700.0, 4, 700.0, 0.0, 0.35),
            ProvinceKind::PassiveMargin => (-400.0, 3, 1200.0, 0.0, 0.0),
            ProvinceKind::SedimentaryBasin => (-550.0, 3, 1800.0, 0.05, 0.0),
            ProvinceKind::Craton => (120.0, 1, 200.0, 0.0, 0.05),
            ProvinceKind::ColdPhase => (0.0, 1, 0.0, 0.0, 0.0),
        };
        provinces.push(Province {
            id,
            kind,
            footprint,
            age_ma: if kind == ProvinceKind::ColdPhase {
                0.05 + 0.15 * rng.unit()
            } else {
                80.0 + 900.0 * rng.unit()
            },
            initial_relief_m: relief,
            basement_lithology_id: rock,
            sediment_cover_m: sediment,
            folding: fold,
            intrusion_fraction: intrusion,
            mineralization: intrusion * 0.3,
        });
    }
    let boundaries = extract_boundaries(grid, &plate_id, &plates, &crust);
    let mut hotspots = Vec::new();
    // Sources are sparse in physical area; a small domain need not host a plume.
    let hotspot_count = ((grid.width_m * grid.depth_m) / 80_000_000_000_000.0).floor() as usize;
    for id in 0..hotspot_count {
        let source = Point {
            x_m: grid.min_x_m + rng.unit() * grid.width_m,
            z_m: grid.min_z_m + rng.unit() * grid.depth_m,
        };
        hotspots.push(Hotspot {
            id,
            plate_id: plate_id[grid.index_at(source.x_m, source.z_m)],
            source,
            duration_ma: 50.0,
            interval_ma: 5.0,
            radius_m: 65_000.0,
            initial_height_m: 6500.0,
        });
    }
    assemble(
        grid, plates, plate_id, fragments, crust, boundaries, provinces, hotspots,
    )
}

// Boundary edges lie on actual common cell faces, and are chained once per pair.
fn extract_boundaries(
    grid: &Grid,
    ids: &[usize],
    plates: &[Plate],
    crust: &[CrustCell],
) -> Vec<BoundarySegment> {
    type Vertex = (usize, usize);
    let mut pairs: BTreeMap<(usize, usize), Vec<(Vertex, Vertex)>> = BTreeMap::new();
    for i in 0..grid.len() {
        let x = i % grid.columns;
        let z = i / grid.columns;
        for (j, edge) in [
            (
                if x + 1 < grid.columns {
                    Some(i + 1)
                } else {
                    None
                },
                ((x + 1, z), (x + 1, z + 1)),
            ),
            (
                if z + 1 < grid.rows {
                    Some(i + grid.columns)
                } else {
                    None
                },
                ((x, z + 1), (x + 1, z + 1)),
            ),
        ] {
            if let Some(j) = j
                && ids[i] != ids[j]
            {
                let a = ids[i].min(ids[j]);
                let b = ids[i].max(ids[j]);
                pairs.entry((a, b)).or_default().push(edge);
            }
        }
    }
    let physical = |v: Vertex| Point {
        x_m: grid.min_x_m + (v.0 as f64 * grid.step_m).min(grid.width_m),
        z_m: grid.min_z_m + (v.1 as f64 * grid.step_m).min(grid.depth_m),
    };
    let mut result = Vec::new();
    for ((a, b), edges) in pairs {
        let pa = plates.iter().find(|p| p.id == a).unwrap();
        let pb = plates.iter().find(|p| p.id == b).unwrap();
        let relative = pb.velocity_history[0]
            .velocity_m_per_ma
            .sub(pa.velocity_history[0].velocity_m_per_ma);
        let mut adjacency: BTreeMap<Vertex, Vec<usize>> = BTreeMap::new();
        for (i, (u, v)) in edges.iter().enumerate() {
            adjacency.entry(*u).or_default().push(i);
            adjacency.entry(*v).or_default().push(i);
        }
        let mut used = vec![false; edges.len()];
        while let Some(first) = used.iter().position(|v| !*v) {
            let (u, v) = edges[first];
            let start = if adjacency[&u].len() != 2 {
                u
            } else if adjacency[&v].len() != 2 {
                v
            } else {
                // Find an endpoint in this component before starting, avoiding arbitrary splits.
                let mut at = u;
                let mut previous = first;
                let mut seen = std::collections::BTreeSet::new();
                loop {
                    if !seen.insert(at) || adjacency[&at].len() != 2 {
                        break at;
                    }
                    let next = *adjacency[&at].iter().find(|e| **e != previous).unwrap();
                    let (s, t) = edges[next];
                    at = if at == s { t } else { s };
                    previous = next;
                }
            };
            let mut vertices = vec![start];
            let mut at = start;
            loop {
                let next = adjacency[&at].iter().copied().find(|i| !used[*i]);
                let Some(next) = next else {
                    break;
                };
                used[next] = true;
                let (s, t) = edges[next];
                at = if at == s { t } else { s };
                vertices.push(at);
                if adjacency[&at].len() != 2 || at == start {
                    break;
                }
            }
            let mut points: Vec<Point> = vertices.into_iter().map(physical).collect();
            // Determine orientation from the actual two incident cells, including
            // closed loops; a nucleus-to-nucleus chord cannot orient a closed ring.
            let first_tangent = points[1].sub(points[0]).unit();
            let first_normal = Point {
                x_m: first_tangent.z_m,
                z_m: -first_tangent.x_m,
            };
            let first_midpoint = points[0].add(points[1]).scale(0.5);
            let positive = first_midpoint.add(first_normal.scale(grid.step_m * 0.25));
            let reverse = ids[grid.index_at(positive.x_m, positive.z_m)] == a;
            // Cell-edge stair steps are smoothed in physical space without moving endpoints.
            let mut smoothed = Vec::with_capacity(points.len());
            for _ in 0..2 {
                smoothed.clear();
                smoothed.push(points[0]);
                for w in points.windows(3) {
                    smoothed.push(w[0].add(w[1].scale(2.0)).add(w[2]).scale(0.25));
                }
                smoothed.push(*points.last().unwrap());
                std::mem::swap(&mut points, &mut smoothed);
            }
            if reverse {
                points.reverse();
            }
            // Start a closed contact at a physical interaction transition so
            // a convergent arc crossing the polyline seam is classified once.
            // This locks its polarity across the entire coherent crust/arc run.
            if points[0] == *points.last().unwrap() {
                let mut previous = boundary_interaction_context(
                    &points[points.len() - 2..],
                    grid,
                    crust,
                    relative,
                );
                let mut cut: Option<usize> = None;
                for edge in 0..points.len() - 1 {
                    let next = boundary_interaction_context(
                        &points[edge..edge + 2],
                        grid,
                        crust,
                        relative,
                    );
                    if next != previous
                        && cut.is_none_or(|i| point_order(points[edge], points[i]).is_lt())
                    {
                        cut = Some(edge);
                    }
                    previous = next;
                }
                if let Some(cut) = cut {
                    points.pop();
                    points.rotate_left(cut);
                    points.push(points[0]);
                }
            }
            // A shared contact may cross crust composition AND motion regimes.
            // Never average opposing normal motions around an enclosing plate.
            let mut start = 0;
            let mut context = boundary_interaction_context(&points[0..2], grid, crust, relative);
            for edge in 1..points.len() - 1 {
                let next =
                    boundary_interaction_context(&points[edge..edge + 2], grid, crust, relative);
                if next != context {
                    result.push(classify_segment(
                        result.len(),
                        a,
                        b,
                        points[start..=edge].to_vec(),
                        plates,
                        grid,
                        crust,
                    ));
                    start = edge;
                    context = next;
                }
            }
            result.push(classify_segment(
                result.len(),
                a,
                b,
                points[start..].to_vec(),
                plates,
                grid,
                crust,
            ));
        }
    }
    result
}

fn boundary_interaction_context(
    edge: &[Point],
    grid: &Grid,
    crust: &[CrustCell],
    relative: Point,
) -> (bool, bool, i8, i8) {
    let t = edge[1].sub(edge[0]).unit();
    let n = Point {
        x_m: t.z_m,
        z_m: -t.x_m,
    };
    let m = edge[0].add(edge[1]).scale(0.5);
    let a = m.sub(n.scale(grid.step_m * 0.6));
    let b = m.add(n.scale(grid.step_m * 0.6));
    let motion_class = |v: f64| {
        if v < -MOTION_EPSILON_M_PER_MA {
            -1
        } else if v > MOTION_EPSILON_M_PER_MA {
            1
        } else {
            0
        }
    };
    let normal_class = motion_class(relative.dot(n));
    // Convergent/divergent arcs can change obliquity without changing their
    // slab polarity. Transform runs additionally retain a coherent shear sign.
    let shear_class = if normal_class == 0 {
        motion_class(relative.dot(t))
    } else {
        0
    };
    (
        crust[grid.index_at(a.x_m, a.z_m)].continental_fraction >= 0.5,
        crust[grid.index_at(b.x_m, b.z_m)].continental_fraction >= 0.5,
        normal_class,
        shear_class,
    )
}

fn nearest(points: &[Point], p: Point) -> (f64, f64, usize) {
    let mut best = (f64::INFINITY, 0.0, 0);
    let mut best_edge = (points[0], points[1]);
    for (i, e) in points.windows(2).enumerate() {
        // Project from the physically canonical endpoint, not the A/B-oriented
        // endpoint: equivalent reversed interpolation is not bit-identical.
        let reverse = point_order(e[0], e[1]).is_gt();
        let (start, end) = if reverse { (e[1], e[0]) } else { (e[0], e[1]) };
        let d = end.sub(start);
        let u = (p.sub(start).dot(d) / d.dot(d)).clamp(0.0, 1.0);
        let delta = p.sub(start.add(d.scale(u)));
        let distance = delta.norm();
        let edge_order =
            point_order(start, best_edge.0).then_with(|| point_order(end, best_edge.1));
        if distance < best.0 || (distance == best.0 && edge_order.is_lt()) {
            let n = Point {
                x_m: d.z_m,
                z_m: -d.x_m,
            }
            .unit();
            // Round end caps: using only the normal projection would extend
            // an open segment's ridge forever beyond its endpoint.
            let orientation = if reverse { -1.0 } else { 1.0 };
            best = (distance, distance.copysign(delta.dot(n) * orientation), i);
            best_edge = (start, end);
        }
    }
    best
}
fn classify_segment(
    id: usize,
    a: usize,
    b: usize,
    points: Vec<Point>,
    plates: &[Plate],
    grid: &Grid,
    crust: &[CrustCell],
) -> BoundarySegment {
    let pa = plates.iter().find(|p| p.id == a).unwrap();
    let pb = plates.iter().find(|p| p.id == b).unwrap();
    let relative = pb.velocity_history[0]
        .velocity_m_per_ma
        .sub(pa.velocity_history[0].velocity_m_per_ma);
    let mut normal = 0.0;
    let mut shear = 0.0;
    let mut length = 0.0;
    let mut ca = 0.0;
    let mut cb = 0.0;
    let mut aa = 0.0;
    let mut ab = 0.0;
    // Canonical physical traversal also fixes floating-point reduction order
    // under side/ID exchange, including closed contacts with identical endpoints.
    let endpoint_order = point_order(points[0], *points.last().unwrap());
    let reverse = endpoint_order.is_gt()
        || (endpoint_order.is_eq() && point_order(points[1], points[points.len() - 2]).is_gt());
    for step in 0..points.len() - 1 {
        let edge = if reverse {
            points.len() - 2 - step
        } else {
            step
        };
        let e = &points[edge..edge + 2];
        let d = e[1].sub(e[0]);
        let l = d.norm();
        let t = d.unit();
        let n = Point {
            x_m: t.z_m,
            z_m: -t.x_m,
        };
        let m = e[0].add(e[1]).scale(0.5);
        normal += relative.dot(n) * l;
        shear += relative.dot(t) * l;
        length += l;
        let ia = grid.index_at(
            m.x_m - n.x_m * grid.step_m * 0.6,
            m.z_m - n.z_m * grid.step_m * 0.6,
        );
        let ib = grid.index_at(
            m.x_m + n.x_m * grid.step_m * 0.6,
            m.z_m + n.z_m * grid.step_m * 0.6,
        );
        ca += crust[ia].continental_fraction * l;
        cb += crust[ib].continental_fraction * l;
        aa += crust[ia].ocean_floor_age_ma * l;
        ab += crust[ib].ocean_floor_age_ma * l;
    }
    normal /= length;
    shear /= length;
    ca /= length;
    cb /= length;
    let regime = if normal < -MOTION_EPSILON_M_PER_MA {
        if ca >= 0.5 && cb >= 0.5 {
            BoundaryRegime::Collision
        } else {
            BoundaryRegime::Subduction
        }
    } else if normal > MOTION_EPSILON_M_PER_MA {
        if ca < 0.5 && cb < 0.5 {
            BoundaryRegime::OceanicSpreading
        } else {
            BoundaryRegime::ContinentalRift
        }
    } else if shear.abs() > MOTION_EPSILON_M_PER_MA {
        BoundaryRegime::Transform
    } else {
        BoundaryRegime::Passive
    };
    let subducting_plate = if regime == BoundaryRegime::Subduction {
        Some(if ca < 0.5 && cb >= 0.5 {
            a
        } else if cb < 0.5 && ca >= 0.5 {
            b
        } else if aa > ab
            || (aa == ab
                && pa
                    .nucleus
                    .x_m
                    .total_cmp(&pb.nucleus.x_m)
                    .then_with(|| pa.nucleus.z_m.total_cmp(&pb.nucleus.z_m))
                    .is_lt())
        {
            a
        } else {
            b
        })
    } else {
        None
    };
    let chord = points.last().unwrap().sub(points[0]);
    let mean_normal = Point {
        x_m: chord.z_m / length,
        z_m: -chord.x_m / length,
    };
    let backarc_extension = subducting_plate.is_some_and(|slab| {
        let (upper, normal_to_upper) = if slab == a {
            (pb, mean_normal)
        } else {
            (pa, mean_normal.scale(-1.0))
        };
        upper.velocity_history[0]
            .velocity_m_per_ma
            .dot(normal_to_upper)
            > 10_000.0
    });
    BoundarySegment {
        id,
        plate_a: a,
        plate_b: b,
        points,
        relative_velocity_m_per_ma: relative,
        normal_motion_m_per_ma: normal,
        shear_m_per_ma: shear,
        regime,
        subducting_plate,
        backarc_extension,
        duration_ma: 25.0,
    }
}

/// Gaussian physical-distance kernel: explicitly empirical, no claim of exact mechanics.
fn empirical_band(distance_m: f64, center_m: f64, width_m: f64) -> f64 {
    (-0.5 * ((distance_m - center_m) / width_m).powi(2)).exp()
}
fn thermal_depth_proxy(age_ma: f64) -> f64 {
    -2600.0 - 330.0 * age_ma.min(180.0).sqrt()
}
fn crust_height_proxy(c: &CrustCell) -> f64 {
    let continental = 250.0 + (c.thickness_m - 35_000.0) * 0.075;
    continental * c.continental_fraction
        + thermal_depth_proxy(c.ocean_floor_age_ma) * (1.0 - c.continental_fraction)
}
fn bend_deformation(b: &BoundarySegment, p: Point) -> f64 {
    let mut uplift = 0.0_f64;
    let mut dominant_corner = Point {
        x_m: f64::INFINITY,
        z_m: f64::INFINITY,
    };
    for w in b.points.windows(3) {
        let t0 = w[1].sub(w[0]).unit();
        let t1 = w[2].sub(w[1]).unit();
        let n0 = Point {
            x_m: t0.z_m,
            z_m: -t0.x_m,
        };
        let n1 = Point {
            x_m: t1.z_m,
            z_m: -t1.x_m,
        };
        // Local normal motion from the two incident tangents gives the
        // geometric sign. Under A/B exchange both normals and relative velocity
        // reverse, preserving the result; curvature sign alone cannot do this.
        let signed =
            (-b.relative_velocity_m_per_ma.dot(n0.add(n1).scale(0.5)) / 40_000.0).clamp(-1.0, 1.0);
        let v = 1100.0 * signed * empirical_band(p.sub(w[1]).norm(), 0.0, 70_000.0);
        if v.abs() > uplift.abs()
            || (v.abs() == uplift.abs() && point_order(w[1], dominant_corner).is_lt())
        {
            uplift = v;
            dominant_corner = w[1];
        }
    }
    uplift
}

/// Backtrace inherited geological bands through the dominant transform fault.
/// This is a finite-width empirical shear-zone advection proxy, not height noise.
fn inherited_material_point(boundaries: &[BoundarySegment], p: Point) -> (Point, f64) {
    let mut displacement = Point { x_m: 0.0, z_m: 0.0 };
    let mut weakness = 0.0_f64;
    for b in boundaries {
        if b.regime != BoundaryRegime::Transform {
            continue;
        }
        let (distance, signed, edge) = nearest(&b.points, p);
        let tangent = b.points[edge + 1].sub(b.points[edge]).unit();
        let slip = 0.5
            * b.shear_m_per_ma
            * b.duration_ma
            * (signed / 15_000.0).tanh()
            * empirical_band(distance, 0.0, 60_000.0);
        let shift = tangent.scale(slip);
        if shift.norm() > displacement.norm() {
            displacement = shift;
        }
        weakness = weakness.max(0.25 * empirical_band(distance, 0.0, 15_000.0));
    }
    (p.sub(displacement), weakness)
}
fn boundary_relief(b: &BoundarySegment, p: Point, cell_sigma_m: f64) -> f64 {
    let (_, signed, _) = nearest(&b.points, p);
    let d = signed;
    let intensity = (b.normal_motion_m_per_ma.abs() / 40_000.0).clamp(0.15, 1.5);
    // Gaussian cell-footprint aggregation matches the physical cell variance.
    // Widening retains each band's signed offset and integrated contribution;
    // a coarse cell does not receive four unattenuated coincident peaks.
    let band = |d: f64, c: f64, w: f64| {
        let effective = (w * w + cell_sigma_m * cell_sigma_m).sqrt();
        w / effective * empirical_band(d, c, effective)
    };
    match b.regime {
        BoundaryRegime::Collision => {
            intensity
                * (2600.0 * band(d, 0.0, 150_000.0) + 900.0 * band(d, 0.0, 40_000.0)
                    - 550.0 * band(d.abs(), 260_000.0, 60_000.0))
        }
        BoundaryRegime::Subduction => {
            let upper = if b.subducting_plate == Some(b.plate_a) {
                d
            } else {
                -d
            };
            intensity
                * (-3200.0 * band(upper, -35_000.0, 22_000.0)
                    + 700.0 * band(upper, 35_000.0, 28_000.0)
                    + 6500.0 * band(upper, 110_000.0, 32_000.0)
                    - if b.backarc_extension {
                        700.0 * band(upper, 200_000.0, 50_000.0)
                    } else {
                        0.0
                    })
        }
        // Young-floor thermal uplift is carried by crust age, not double-added here.
        BoundaryRegime::OceanicSpreading => 450.0 * intensity * band(d, 0.0, 20_000.0),
        BoundaryRegime::ContinentalRift => {
            intensity
                * (-1000.0 * band(d, 0.0, 25_000.0) + 650.0 * band(d.abs(), 70_000.0, 25_000.0))
        }
        BoundaryRegime::Transform => -100.0 * band(d, 0.0, 15_000.0) + bend_deformation(b, p),
        BoundaryRegime::Passive => 0.0,
    }
}
fn chain(h: &Hotspot, p: &Plate) -> Vec<Volcano> {
    let count = (h.duration_ma / h.interval_ma).floor() as usize;
    (0..=count)
        .map(|i| {
            let age_ma = i as f64 * h.interval_ma;
            let mut displacement = Point { x_m: 0.0, z_m: 0.0 };
            for (j, e) in p.velocity_history.iter().enumerate() {
                let end = p
                    .velocity_history
                    .get(j + 1)
                    .map_or(age_ma, |next| next.start_age_ma.min(age_ma));
                let duration = (end - e.start_age_ma).max(0.0);
                displacement = displacement.add(e.velocity_m_per_ma.scale(duration));
            }
            Volcano {
                hotspot_id: h.id,
                plate_id: p.id,
                center: h.source.add(displacement),
                age_ma,
                radius_m: h.radius_m,
                height_m: h.initial_height_m * (-age_ma / 45.0).exp(),
                subsidence_m: 25.0 * age_ma,
            }
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn assemble(
    grid: &Grid,
    plates: Vec<Plate>,
    plate_id: Vec<usize>,
    fragments: Vec<CrustFragment>,
    mut crust: Vec<CrustCell>,
    boundaries: Vec<BoundarySegment>,
    provinces: Vec<Province>,
    hotspots: Vec<Hotspot>,
) -> Result<TectonicPlan, PrepareError> {
    let rocks = lithologies();
    let volcanoes: Vec<Volcano> = hotspots
        .iter()
        .flat_map(|h| chain(h, plates.iter().find(|p| p.id == h.plate_id).unwrap()))
        .collect();
    let structures = boundaries
        .iter()
        .map(|b| {
            let sub = b.regime == BoundaryRegime::Subduction;
            let collision = b.regime == BoundaryRegime::Collision;
            Structure {
                boundary_id: b.id,
                basement_lithology_id: if sub {
                    5
                } else if collision {
                    2
                } else {
                    4
                },
                folding: if collision {
                    0.9
                } else if sub {
                    0.4
                } else {
                    0.0
                },
                fault_displacement_m: if b.regime == BoundaryRegime::Transform {
                    b.shear_m_per_ma * b.duration_ma
                } else {
                    b.normal_motion_m_per_ma * b.duration_ma
                },
                intrusion_fraction: if sub {
                    0.65
                } else if b.regime == BoundaryRegime::OceanicSpreading {
                    0.9
                } else {
                    0.05
                },
                mineralization: if sub { 0.7 } else { 0.1 },
                trench_offset_m: if sub { -35_000.0 } else { 0.0 },
                forearc_offset_m: if sub { 35_000.0 } else { 0.0 },
                arc_offset_m: if sub { 110_000.0 } else { 0.0 },
                backarc_offset_m: if b.backarc_extension { 200_000.0 } else { 0.0 },
            }
        })
        .collect();
    let mut elevation_m = Vec::with_capacity(grid.len());
    let mut erodibility = Vec::with_capacity(grid.len());
    let mut oceanic = Vec::with_capacity(grid.len());
    let mut cold_phase = Vec::with_capacity(grid.len());
    for i in 0..grid.len() {
        let p = point(grid, i);
        let c = &mut crust[i];
        let mut thickening_m = 0.0_f64;
        let mut thinning_m = 0.0_f64;
        for b in &boundaries {
            if b.regime == BoundaryRegime::OceanicSpreading && c.continental_fraction < 0.5 {
                let (d, _, _) = nearest(&b.points, p);
                // Distance / half-spreading rate is a bounded floor-age proxy.
                let age = d / (b.normal_motion_m_per_ma * 0.5).max(MOTION_EPSILON_M_PER_MA);
                c.ocean_floor_age_ma = c.ocean_floor_age_ma.min(age);
            }
            if c.continental_fraction >= 0.5
                && matches!(
                    b.regime,
                    BoundaryRegime::Collision
                        | BoundaryRegime::ContinentalRift
                        | BoundaryRegime::Subduction
                )
            {
                let (_, signed, _) = nearest(&b.points, p);
                let intensity = (b.normal_motion_m_per_ma.abs() / 40_000.0).clamp(0.15, 1.5);
                if b.regime == BoundaryRegime::Collision {
                    let thickening = 12_000.0 * intensity * empirical_band(signed, 0.0, 150_000.0);
                    thickening_m = thickening_m.max(thickening);
                    if thickening > 5000.0 {
                        c.lithology_id = 2;
                    }
                } else if b.regime == BoundaryRegime::ContinentalRift {
                    thinning_m =
                        thinning_m.max(3000.0 * intensity * empirical_band(signed, 0.0, 25_000.0));
                } else if b.regime == BoundaryRegime::Subduction {
                    let upper = if b.subducting_plate == Some(b.plate_a) {
                        signed
                    } else {
                        -signed
                    };
                    if empirical_band(upper, 110_000.0, 32_000.0) > 0.5 {
                        c.lithology_id = 5;
                    }
                }
            }
        }
        c.thickness_m += thickening_m - thinning_m;
        if c.continental_fraction < 0.5 {
            c.basement_age_ma = c.ocean_floor_age_ma;
        }
        let (material_point, fault_weakness) = inherited_material_point(&boundaries, p);
        let mut inherited = 0.0;
        let mut cold = false;
        for province in &provinces {
            let r = province
                .footprint
                .radius(if province.kind == ProvinceKind::ColdPhase {
                    p
                } else {
                    material_point
                });
            if r > 3.0 {
                continue;
            }
            let w = (-2.0 * r * r).exp();
            if province.kind == ProvinceKind::ColdPhase {
                cold |= r <= 1.0;
                continue;
            }
            // Wear attenuates relief; hard lithology retains more residual relief.
            let resistance = 1.0 - rocks[province.basement_lithology_id].erodibility;
            let age_factor = (-province.age_ma / (300.0 + 700.0 * resistance)).exp();
            inherited += province.initial_relief_m * w * age_factor;
            c.sediment_cover_m += province.sediment_cover_m * w;
            if w > 0.5 {
                c.lithology_id = province.basement_lithology_id;
                c.basement_age_ma = c.basement_age_ma.max(province.age_ma);
            }
        }
        // At junctions retain dominant uplift and subsidence, never add each edge.
        let mut uplift = 0.0_f64;
        let mut subsidence = 0.0_f64;
        let cell_sigma_m = (grid.area(i) / 12.0).sqrt();
        for b in &boundaries {
            let v = boundary_relief(b, p, cell_sigma_m);
            uplift = uplift.max(v);
            subsidence = subsidence.min(v);
        }
        let mut volcanic = 0.0_f64;
        for v in &volcanoes {
            if plate_id[i] != v.plate_id {
                continue;
            }
            let height = (v.height_m - v.subsidence_m).max(0.0)
                * empirical_band(p.sub(v.center).norm(), 0.0, v.radius_m);
            volcanic = volcanic.max(height);
            if height > 100.0 {
                c.lithology_id = 0;
            }
        }
        let base = crust_height_proxy(c);
        elevation_m.push(base + inherited + uplift + subsidence + volcanic);
        let sediment_fraction = (c.sediment_cover_m / 500.0).clamp(0.0, 1.0);
        erodibility.push(
            (rocks[c.lithology_id].erodibility * (1.0 - sediment_fraction)
                + 0.85 * sediment_fraction
                + fault_weakness)
                .min(1.0),
        );
        oceanic.push(c.continental_fraction < 0.5);
        cold_phase.push(cold);
    }
    let plan = TectonicPlan {
        elevation_m,
        erodibility,
        oceanic,
        cold_phase,
        plate_id,
        crust,
        plates,
        fragments,
        boundaries,
        provinces,
        lithologies: rocks,
        structures,
        hotspots,
        volcanoes,
    };
    plan.validate(grid)?;
    Ok(plan)
}

#[cfg(test)]
#[path = "tectonics_tests.rs"]
mod tests;
