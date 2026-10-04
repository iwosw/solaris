//! Root-indexed, immutable water footprints and valley geometry in model metres.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::definition::WorldIdentity;
use super::grid::Grid;
use super::hydrology::{HydrologyPlan, SECONDS_PER_SEASON};
use super::prepared::PrepareError;
use super::world_plan::PlanePlan;

/// One root-cell-sized query plus a two-cell halo. No catalog/upstream scan.
pub const MAX_CANDIDATE_CELLS: usize = 49;
const ARC_SEGMENTS: usize = 16;
/// Each candidate has at most one stem and three distributaries.
pub const MAX_REACH_FEATURES: usize = MAX_CANDIDATE_CELLS * 4;

/// Root publication only: four linear connected-component passes. Region
/// queries consume the persisted owner raster and never traverse body members.
/// Ocean components remain distinct from connected equal-level lake pools.
pub fn root_water_owners(
    grid: &Grid,
    ocean: &[bool],
    hydrology: &HydrologyPlan,
) -> Result<Vec<[Option<usize>; 4]>, PrepareError> {
    grid.validate()?;
    let n = grid.len();
    if ocean.len() != n || hydrology.seasonal_water_level_m.len() != n {
        return Err(PrepareError::Invalid(
            "root water ownership length mismatch",
        ));
    }
    let mut owners = vec![[None; 4]; n];
    let mut queue = Vec::with_capacity(n);
    for season in [0, 1, 2, 3] {
        for i in 0..n {
            if owners[i][season].is_some() {
                continue;
            }
            let level = if ocean[i] {
                Some(0.0)
            } else {
                hydrology.seasonal_water_level_m[i][season]
            };
            let Some(level) = level else {
                continue;
            };
            if !level.is_finite() {
                return Err(PrepareError::Invalid("nonfinite root water body level"));
            }
            // Iteration starts at the smallest unvisited cell in a component.
            owners[i][season] = Some(i);
            queue.clear();
            queue.push(i);
            let mut next = 0;
            while next < queue.len() {
                let cell = queue[next];
                next += 1;
                for neighbour in grid.neighbours(cell) {
                    if owners[neighbour][season].is_some() || ocean[neighbour] != ocean[i] {
                        continue;
                    }
                    let neighbour_level = if ocean[neighbour] {
                        Some(0.0)
                    } else {
                        hydrology.seasonal_water_level_m[neighbour][season]
                    };
                    if neighbour_level == Some(level) {
                        owners[neighbour][season] = Some(i);
                        queue.push(neighbour);
                    }
                }
            }
        }
    }
    Ok(owners)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hydroperiod {
    pub wet_seasons: [bool; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum WaterPlan {
    None,
    Ocean {
        body_id: [u8; 32],
        surface_m: f64,
        bed_m: f64,
        hydroperiod: Hydroperiod,
    },
    Lake {
        body_id: [u8; 32],
        surface_m: f64,
        bed_m: f64,
        hydroperiod: Hydroperiod,
    },
    River {
        reach_id: [u8; 32],
        surface_m: f64,
        bed_m: f64,
        flow_direction: [f64; 2],
        hydroperiod: Hydroperiod,
        seasonal_q_m3_s: [f64; 4],
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterSample {
    pub ground_m: f64,
    pub water: WaterPlan,
    pub protected: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum MouthRegime {
    None,
    Delta,
    /// Static widened mouth only; this is not a tidal-flow simulation.
    StaticEstuary,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RiverPoint {
    pub x_m: f64,
    pub z_m: f64,
    pub surface_m: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct DeltaFootprint {
    pub split_m: [f64; 2],
    pub radius_m: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RiverReach {
    pub reach_id: [u8; 32],
    pub source_cell: usize,
    pub receiving_cell: usize,
    pub points: [RiverPoint; ARC_SEGMENTS + 1],
    pub width_m: f64,
    pub depth_m: f64,
    pub valley_width_m: f64,
    pub seasonal_q_m3_s: [f64; 4],
    pub hydroperiod: Hydroperiod,
    pub mouth_regime: MouthRegime,
    /// Distributaries reference the same root-owned split, never a region ID.
    pub split_id: Option<[u8; 32]>,
    pub seasonal_fraction: [f64; 4],
    pub sediment_footprint: Option<DeltaFootprint>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterFootprint {
    pub root_cell: usize,
    pub bounds_m: [f64; 4],
    pub ocean: bool,
    /// Common collapsed depression owner, not a list of its entire footprint.
    pub depression_id: Option<usize>,
    pub body_id: [u8; 32],
    pub seasonal_surface_m: [Option<f64>; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaterGeometry {
    pub candidate_cells: usize,
    pub reaches: Vec<RiverReach>,
    pub footprints: Vec<WaterFootprint>,
    grid: Grid,
}

impl WaterGeometry {
    pub fn feature_count(&self) -> usize {
        self.reaches.len() + self.footprints.len()
    }
}

/// Queries are contained in one macro patch; ancestors have separate patches.
/// All arc endpoints, profiles, IDs and clipping are independent of query bounds.
pub fn prepare(
    root: &PlanePlan,
    identity: WorldIdentity,
    bounds: [f64; 4],
) -> Result<WaterGeometry, PrepareError> {
    let grid = &root.grid;
    grid.validate()?;
    // Canonical fractional-coordinate endpoints can lose a few ulps on subtraction.
    // This tolerance never relaxes the independent candidate/feature count bound.
    let span_limit = grid.step_m
        + 8.0 * f64::EPSILON * bounds.into_iter().map(f64::abs).fold(grid.step_m, f64::max);
    if !bounds.into_iter().all(f64::is_finite)
        || bounds[0] > bounds[1]
        || bounds[2] > bounds[3]
        || bounds[1] - bounds[0] > span_limit
        || bounds[3] - bounds[2] > span_limit
        || bounds[0] < grid.min_x_m
        || bounds[1] > grid.min_x_m + grid.width_m
        || bounds[2] < grid.min_z_m
        || bounds[3] > grid.min_z_m + grid.depth_m
    {
        return Err(PrepareError::Invalid("water query exceeds one macro patch"));
    }
    let n = grid.len();
    if [
        root.elevation_m.len(),
        root.ocean.len(),
        root.water_body_owner.len(),
        root.hydrology.downstream.len(),
        root.hydrology.discharge_m3.len(),
        root.hydrology.depression.len(),
        root.hydrology.seasonal_water_level_m.len(),
        root.tectonics.erodibility.len(),
        root.climate_forcing.wind_m_s.len(),
    ]
    .into_iter()
    .any(|length| length != n)
    {
        return Err(PrepareError::Invalid("water root array length mismatch"));
    }
    let first = grid.index_at(bounds[0], bounds[2]);
    let last = grid.index_at(bounds[1], bounds[3]);
    let min_x = (first % grid.columns).saturating_sub(2);
    let max_x = (last % grid.columns)
        .saturating_add(2)
        .min(grid.columns - 1);
    let min_z = (first / grid.columns).saturating_sub(2);
    let max_z = (last / grid.columns).saturating_add(2).min(grid.rows - 1);
    let candidate_cells = (max_x - min_x + 1) * (max_z - min_z + 1);
    if candidate_cells > MAX_CANDIDATE_CELLS {
        return Err(PrepareError::Invalid("water candidate limit exceeded"));
    }
    let mut geometry = WaterGeometry {
        candidate_cells,
        reaches: Vec::with_capacity(candidate_cells * 4),
        footprints: Vec::with_capacity(candidate_cells),
        grid: grid.clone(),
    };
    for z in min_z..=max_z {
        for x in min_x..=max_x {
            let i = z * grid.columns + x;
            if !root.elevation_m[i].is_finite() {
                return Err(PrepareError::Invalid("nonfinite local water elevation"));
            }
            let levels = if root.ocean[i] {
                [Some(0.0); 4]
            } else {
                root.hydrology.seasonal_water_level_m[i]
            };
            if root.ocean[i] || levels.iter().any(Option::is_some) {
                let depression = root.hydrology.depression[i];
                let owner = root.water_body_owner[i]
                    .into_iter()
                    .rev()
                    .flatten()
                    .next()
                    .ok_or(PrepareError::Invalid(
                        "wet local footprint has no root body owner",
                    ))?;
                let body_id = feature_id(
                    identity,
                    if root.ocean[i] { b"ocean" } else { b"lake" },
                    owner,
                    0,
                );
                geometry.footprints.push(WaterFootprint {
                    root_cell: i,
                    bounds_m: cell_bounds(grid, i),
                    ocean: root.ocean[i],
                    depression_id: depression,
                    body_id,
                    seasonal_surface_m: levels,
                });
            }
            build_reaches(root, identity, i, &mut geometry.reaches)?;
        }
    }
    Ok(geometry)
}

fn feature_id(identity: WorldIdentity, tag: &[u8], a: usize, b: usize) -> [u8; 32] {
    let mut hash = Sha256::new();
    hash.update(b"Solaris-W3-water\0");
    hash.update(identity.bytes());
    hash.update(tag);
    hash.update((a as u64).to_be_bytes());
    hash.update((b as u64).to_be_bytes());
    hash.finalize().into()
}

fn cell_bounds(grid: &Grid, i: usize) -> [f64; 4] {
    let x = grid.min_x_m + (i % grid.columns) as f64 * grid.step_m;
    let z = grid.min_z_m + (i / grid.columns) as f64 * grid.step_m;
    [
        x,
        (x + grid.step_m).min(grid.min_x_m + grid.width_m),
        z,
        (z + grid.step_m).min(grid.min_z_m + grid.depth_m),
    ]
}

fn anchor(root: &PlanePlan, i: usize) -> RiverPoint {
    let (x_m, z_m) = root.grid.center(i);
    let surface_m = if root.ocean[i] {
        0.0
    } else {
        root.hydrology.seasonal_water_level_m[i][3].unwrap_or(root.elevation_m[i])
    };
    RiverPoint {
        x_m,
        z_m,
        surface_m,
    }
}

fn interpolate(a: &RiverPoint, b: &RiverPoint, t: f64) -> RiverPoint {
    if t == 0.0 {
        return *a;
    }
    if t == 1.0 {
        return *b;
    }
    RiverPoint {
        x_m: a.x_m + (b.x_m - a.x_m) * t,
        z_m: a.z_m + (b.z_m - a.z_m) * t,
        surface_m: a.surface_m + (b.surface_m - a.surface_m) * t,
    }
}

fn shared_edge(grid: &Grid, i: usize, j: usize) -> [f64; 2] {
    let (ix, iz) = grid.center(i);
    let (jx, jz) = grid.center(j);
    let bounds = cell_bounds(grid, i);
    if jx > ix {
        [bounds[1], iz]
    } else if jx < ix {
        [bounds[0], iz]
    } else if jz > iz {
        [ix, bounds[3]]
    } else {
        [ix, bounds[2]]
    }
}

fn build_reaches(
    root: &PlanePlan,
    identity: WorldIdentity,
    i: usize,
    reaches: &mut Vec<RiverReach>,
) -> Result<(), PrepareError> {
    let Some(j) = root.hydrology.downstream[i] else {
        return Ok(());
    };
    if j >= root.grid.len() || !root.grid.neighbours(i).any(|cell| cell == j) {
        return Err(PrepareError::Invalid(
            "water edge is not a local root neighbour",
        ));
    }
    let volumes = root.hydrology.discharge_m3[i];
    if !volumes.into_iter().all(|q| q.is_finite() && q >= 0.0) {
        return Err(PrepareError::Invalid("invalid local water discharge"));
    }
    if root.ocean[i] || volumes[3] == 0.0 {
        return Ok(());
    }
    if let Some(d) = root.hydrology.depression[i] {
        let depression = root
            .hydrology
            .depressions
            .get(d)
            .ok_or(PrepareError::Invalid("unknown local depression"))?;
        // Collapsed reservoir transport edges are not excavated rivers.
        if i != depression.spill_from {
            return Ok(());
        }
    }
    let mut a = anchor(root, i);
    let mut b = anchor(root, j);
    let source_body = root.hydrology.seasonal_water_level_m[i][3].is_some();
    let receiving = root.ocean[j] || root.hydrology.seasonal_water_level_m[j][3].is_some();
    if source_body || receiving {
        let edge = shared_edge(&root.grid, i, j);
        if source_body {
            a.x_m = edge[0];
            a.z_m = edge[1];
        }
        if receiving {
            b.x_m = edge[0];
            b.z_m = edge[1];
        }
    }
    // Adjacent horizontal bodies own their common shoreline; their conceptual
    // transfer edge is not another open river through lake/ocean water.
    if source_body && receiving {
        return Ok(());
    }
    if a.surface_m < b.surface_m || !b.surface_m.is_finite() {
        return Err(PrepareError::Invalid("river anchor runs uphill"));
    }
    let length = (b.x_m - a.x_m).hypot(b.z_m - a.z_m);
    let slope = (a.surface_m - b.surface_m) / length;
    let material = root.tectonics.erodibility[i];
    if !material.is_finite() || !(0.0..=1.0).contains(&material) {
        return Err(PrepareError::Invalid("invalid local river material"));
    }
    let seasonal_q_m3_s = volumes.map(|q| q / SECONDS_PER_SEASON);
    let q = seasonal_q_m3_s.iter().sum::<f64>() * 0.25;
    // Empirical hydraulic-geometry proxy: wider alluvium, narrower steep rock.
    // The only upper bound is physical macro-corridor confinement, not blocks.
    let width = (2.5 * q.sqrt() * (0.5 + material) / (1.0 + 15.0 * slope.sqrt()))
        .clamp((0.5_f64).min(length * 0.05), length * 0.3);
    let depth = (0.15 + q.powf(0.3) * (0.4 + 0.4 * material))
        .min(width * 0.5)
        .max(width * 0.1);
    let valley = (width * (2.0 + 6.0 * material / (1.0 + slope * 100.0)))
        .min(length * 0.45)
        .max(width);
    let hydroperiod = Hydroperiod {
        wet_seasons: volumes.map(|q| q > 0.0),
    };
    let reach_id = feature_id(identity, b"reach", i, j);
    let sediment = root
        .erosion_passes
        .last()
        .and_then(|pass| pass.transported_m3.get(i))
        .map_or(0.0, |load| load[0] + load[1]);
    let depth_receiving = (b.surface_m - root.elevation_m[j]).max(0.0);
    let wind = root.climate_forcing.wind_m_s[j]
        .iter()
        .map(|v| v[0].hypot(v[1]))
        .sum::<f64>()
        * 0.25;
    let sediment_radius = (sediment / (std::f64::consts::PI * depth_receiving.max(1.0))).sqrt();
    let delta = receiving
        && sediment > volumes.iter().sum::<f64>() * 0.00001
        && depth_receiving > 0.0
        && depth_receiving <= 15.0
        && wind < 8.0
        && sediment_radius > width * 2.0;
    let mouth_regime = if delta {
        MouthRegime::Delta
    } else if receiving {
        MouthRegime::StaticEstuary
    } else {
        MouthRegime::None
    };
    let mut stem = RiverReach {
        reach_id,
        source_cell: i,
        receiving_cell: j,
        points: [a; ARC_SEGMENTS + 1],
        width_m: if mouth_regime == MouthRegime::StaticEstuary {
            width * 1.5
        } else {
            width
        },
        depth_m: depth,
        valley_width_m: valley,
        seasonal_q_m3_s,
        hydroperiod,
        mouth_regime,
        split_id: None,
        seasonal_fraction: [1.0; 4],
        sediment_footprint: None,
    };
    if delta {
        // Shared split is the shoreline at the receiving body's level. A fan
        // stays in the receiving cell and within the sediment-supported radius.
        let split = b;
        let split_id = feature_id(identity, b"delta-split", i, j);
        stem.points = arc(root, i, j, &a, &split, 0.0)?;
        reaches.push(stem);
        let receiving_bounds = cell_bounds(&root.grid, j);
        let fan_radius = (sediment_radius * 0.5)
            .min(root.grid.step_m * 0.18)
            .min((receiving_bounds[1] - receiving_bounds[0]) * 0.3)
            .min((receiving_bounds[3] - receiving_bounds[2]) * 0.3);
        let receiving_center = anchor(root, j);
        let fan_length = ((receiving_center.x_m - split.x_m)
            .hypot(receiving_center.z_m - split.z_m))
        .min((sediment_radius * sediment_radius - fan_radius * fan_radius).sqrt());
        let direction = [(split.x_m - a.x_m) / length, (split.z_m - a.z_m) / length];
        let normal = [-direction[1], direction[0]];
        for (branch, (offset, fraction)) in [(-1.0, 0.25_f64), (0.0, 0.5), (1.0, 0.25)]
            .into_iter()
            .enumerate()
        {
            let mut end = split;
            end.x_m += direction[0] * fan_length + normal[0] * offset * fan_radius;
            end.z_m += direction[1] * fan_length + normal[1] * offset * fan_radius;
            let branch_width = width * fraction.sqrt();
            reaches.push(RiverReach {
                reach_id: feature_id(identity, b"delta-branch", i, branch),
                source_cell: i,
                receiving_cell: j,
                points: arc(root, i, j, &split, &end, 0.0)?,
                width_m: branch_width,
                depth_m: depth,
                valley_width_m: branch_width * 1.5,
                seasonal_q_m3_s: seasonal_q_m3_s.map(|q| q * fraction),
                hydroperiod,
                mouth_regime,
                split_id: Some(split_id),
                seasonal_fraction: [fraction; 4],
                sediment_footprint: Some(DeltaFootprint {
                    split_m: [split.x_m, split.z_m],
                    radius_m: sediment_radius,
                }),
            });
        }
    } else {
        // Exact piecewise-linear arc lives in the union of the two routed
        // cells. Checking every segment proves full-arc corridor containment:
        // each rectangle is convex and a segment crosses their shared edge once.
        let phase = f64::from(u16::from_be_bytes([reach_id[0], reach_id[1]])) / 65535.0;
        let source_bounds = cell_bounds(&root.grid, i);
        let receiving_bounds = cell_bounds(&root.grid, j);
        let transverse = if a.x_m == b.x_m {
            (source_bounds[1] - source_bounds[0]).min(receiving_bounds[1] - receiving_bounds[0])
        } else {
            (source_bounds[3] - source_bounds[2]).min(receiving_bounds[3] - receiving_bounds[2])
        };
        let amplitude = (length * 0.12).min(transverse * 0.2) * material / (1.0 + slope * 100.0)
            * if phase < 0.5 { -1.0 } else { 1.0 };
        stem.points = constrained_arc(root, i, j, &a, &b, amplitude, depth * 4.0 + 2.0)?;
        reaches.push(stem);
    }
    Ok(())
}

/// Compare the entire piecewise-linear curve with its root thalweg. Bilinear
/// terrain along each straight segment is quadratic, so endpoints plus its
/// stationary point give the maximum extra incision exactly (not endpoints
/// alone). A rejected meander becomes the same canonical straight valley.
fn constrained_arc(
    root: &PlanePlan,
    i: usize,
    j: usize,
    a: &RiverPoint,
    b: &RiverPoint,
    amplitude: f64,
    allowed_extra_cut: f64,
) -> Result<[RiverPoint; ARC_SEGMENTS + 1], PrepareError> {
    let points = arc(root, i, j, a, b, amplitude)?;
    // Each segment remains in one bilinear root-center rectangle. The bend
    // changes sides only at the midpoint, which is itself an arc vertex.
    for (k, edge) in points.windows(2).enumerate() {
        let cost = |t: f64| {
            let point = interpolate(&edge[0], &edge[1], t);
            let straight = interpolate(a, b, (k as f64 + t) / ARC_SEGMENTS as f64);
            terrain_height(root, point.x_m, point.z_m)
                - terrain_height(root, straight.x_m, straight.z_m)
        };
        let y0 = cost(0.0);
        let y1 = cost(1.0);
        let ym = cost(0.5);
        let quadratic = 2.0 * (y1 + y0 - 2.0 * ym);
        let linear = y1 - y0 - quadratic;
        let vertex = if quadratic < 0.0 {
            (-linear / (2.0 * quadratic)).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut max_cost = y0.max(y1);
        if vertex > 0.0 && vertex < 1.0 {
            max_cost = max_cost.max(cost(vertex));
        }
        if max_cost > allowed_extra_cut {
            return arc(root, i, j, a, b, 0.0);
        }
    }
    Ok(points)
}

fn terrain_height(root: &PlanePlan, x_m: f64, z_m: f64) -> f64 {
    let grid = &root.grid;
    let cell = grid.index_at(x_m, z_m);
    let x = cell % grid.columns;
    let z = cell / grid.columns;
    let (cx, cz) = grid.center(cell);
    let x0 = if x_m < cx { x.saturating_sub(1) } else { x };
    let x1 = if x_m < cx {
        x
    } else {
        (x + 1).min(grid.columns - 1)
    };
    let z0 = if z_m < cz { z.saturating_sub(1) } else { z };
    let z1 = if z_m < cz {
        z
    } else {
        (z + 1).min(grid.rows - 1)
    };
    let (left, _) = grid.center(z0 * grid.columns + x0);
    let (right, _) = grid.center(z0 * grid.columns + x1);
    let (_, top) = grid.center(z0 * grid.columns + x0);
    let (_, bottom) = grid.center(z1 * grid.columns + x0);
    let tx = if x0 == x1 {
        0.0
    } else {
        ((x_m - left) / (right - left)).clamp(0.0, 1.0)
    };
    let tz = if z0 == z1 {
        0.0
    } else {
        ((z_m - top) / (bottom - top)).clamp(0.0, 1.0)
    };
    let h00 = root.elevation_m[z0 * grid.columns + x0];
    let h10 = root.elevation_m[z0 * grid.columns + x1];
    let h01 = root.elevation_m[z1 * grid.columns + x0];
    let h11 = root.elevation_m[z1 * grid.columns + x1];
    let upper = h00 + tx * (h10 - h00);
    let lower = h01 + tx * (h11 - h01);
    upper + tz * (lower - upper)
}

fn arc(
    root: &PlanePlan,
    i: usize,
    j: usize,
    a: &RiverPoint,
    b: &RiverPoint,
    amplitude: f64,
) -> Result<[RiverPoint; ARC_SEGMENTS + 1], PrepareError> {
    let length = (b.x_m - a.x_m).hypot(b.z_m - a.z_m);
    let normal = [-(b.z_m - a.z_m) / length, (b.x_m - a.x_m) / length];
    let mut points = [*a; ARC_SEGMENTS + 1];
    for (k, slot) in points.iter_mut().enumerate() {
        let t = k as f64 / ARC_SEGMENTS as f64;
        let mut point = interpolate(a, b, t);
        // Polynomial envelope is exactly zero at shared confluence anchors.
        let bend = amplitude * 16.0 * t * t * (1.0 - t) * (1.0 - t) * (2.0 * t - 1.0);
        point.x_m += normal[0] * bend;
        point.z_m += normal[1] * bend;
        let cell = root.grid.index_at(point.x_m, point.z_m);
        if cell != i && cell != j {
            return Err(PrepareError::Invalid(
                "river arc escaped its root valley corridor",
            ));
        }
        *slot = point;
    }
    Ok(points)
}

/// Carve protected geometry after detail terrain. Never invent water from a
/// negative elevation alone. Dry seasons remain explicit in the hydroperiod.
pub fn sample(geometry: &WaterGeometry, x_m: f64, z_m: f64, ground_m: f64) -> WaterSample {
    let mut result = WaterSample {
        ground_m,
        water: WaterPlan::None,
        protected: false,
    };
    if !x_m.is_finite()
        || !z_m.is_finite()
        || !ground_m.is_finite()
        || x_m < geometry.grid.min_x_m
        || x_m > geometry.grid.min_x_m + geometry.grid.width_m
        || z_m < geometry.grid.min_z_m
        || z_m > geometry.grid.min_z_m + geometry.grid.depth_m
    {
        return result;
    }
    let cell = geometry.grid.index_at(x_m, z_m);
    let mut river: Option<(&RiverReach, f64, f64, [f64; 2])> = None;
    for reach in &geometry.reaches {
        if cell != reach.source_cell && cell != reach.receiving_cell {
            continue;
        }
        if let Some(footprint) = reach.sediment_footprint
            && (cell != reach.receiving_cell
                || (x_m - footprint.split_m[0]).hypot(z_m - footprint.split_m[1])
                    > footprint.radius_m)
        {
            continue;
        }
        for edge in reach.points.windows(2) {
            let a = &edge[0];
            let b = &edge[1];
            let dx = b.x_m - a.x_m;
            let dz = b.z_m - a.z_m;
            let length2 = dx * dx + dz * dz;
            let t = (((x_m - a.x_m) * dx + (z_m - a.z_m) * dz) / length2).clamp(0.0, 1.0);
            let px = a.x_m + t * dx;
            let pz = a.z_m + t * dz;
            let distance = (x_m - px).hypot(z_m - pz);
            if distance > reach.valley_width_m * 0.5 {
                continue;
            }
            let surface = a.surface_m + t * (b.surface_m - a.surface_m);
            let channel_half = reach.width_m * 0.5;
            let bed = surface - reach.depth_m;
            let valley_half = reach.valley_width_m * 0.5;
            if distance <= channel_half {
                result.ground_m = result.ground_m.min(bed);
                result.protected = true;
                // Closest normalized channel, then root-owned ID at junctions.
                let score = distance / channel_half;
                let replace = river.as_ref().is_none_or(|(old, _, old_score, _)| {
                    score < *old_score || (score == *old_score && reach.reach_id < old.reach_id)
                });
                if replace {
                    let length = length2.sqrt();
                    river = Some((reach, surface, score, [dx / length, dz / length]));
                }
            } else if valley_half > channel_half {
                let weight = (valley_half - distance) / (valley_half - channel_half);
                // Sloping valley/floodplain, not an unrelated random wet stripe.
                let target = surface + reach.depth_m * 2.0;
                if result.ground_m > target {
                    result.ground_m = result
                        .ground_m
                        .min(ground_m - (ground_m - target) * weight * weight);
                }
                result.protected = true;
            }
        }
    }
    // Ocean/lake owns receiving cells; river may cut its inlet/outlet bed but
    // cannot substitute its sloping profile for the horizontal body surface.
    if let Some(footprint) = geometry.footprints.iter().find(|p| p.root_cell == cell) {
        result.protected = true;
        if let Some(surface_m) = footprint.seasonal_surface_m[3]
            && result.ground_m < surface_m
        {
            let hydroperiod = Hydroperiod {
                wet_seasons: footprint
                    .seasonal_surface_m
                    .map(|h| h.is_some_and(|h| result.ground_m < h)),
            };
            result.water = if footprint.ocean {
                WaterPlan::Ocean {
                    body_id: footprint.body_id,
                    surface_m,
                    bed_m: result.ground_m,
                    hydroperiod,
                }
            } else {
                WaterPlan::Lake {
                    body_id: footprint.body_id,
                    surface_m,
                    bed_m: result.ground_m,
                    hydroperiod,
                }
            };
            return result;
        }
    }
    if let Some((reach, surface_m, _, flow_direction)) = river {
        result.water = WaterPlan::River {
            reach_id: reach.reach_id,
            surface_m,
            bed_m: result.ground_m,
            flow_direction,
            hydroperiod: reach.hydroperiod,
            seasonal_q_m3_s: reach.seasonal_q_m3_s,
        };
    }
    result
}

#[cfg(test)]
#[path = "region_water_tests.rs"]
mod tests;
