//! Four equal-duration model seasons in physical metres; no voxel mapping inputs.
//! Ocean circulation is a mask-restricted surface heat proxy, not a GCM.
use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use super::grid::Grid;
use super::prepared::PrepareError;

const STEPS: u32 = 128;
const CFL: f64 = 0.4;
const INITIAL_VAPOR_M: f64 = 0.008;
const EXTERNAL_VAPOR_M: f64 = 0.012;
const LAPSE_C_PER_M: f64 = 0.006;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClimateCell {
    pub temperature_c: [f64; 4],
    /// Seasonal accumulated depths, not instantaneous rates.
    pub precipitation_mm: [f64; 4],
    pub potential_et_mm: [f64; 4],
    pub snow_fraction: [f64; 4],
    /// Geographic east/north; physical grid Z points south.
    pub wind_m_s: [[f64; 2]; 4],
    pub ocean: bool,
    /// Zero on land. Land receives coastal influence separately.
    pub sst_c: [f64; 4],
    pub evaporation_mm: [f64; 4],
    pub final_vapor_mm: [f64; 4],
    pub final_condensate_mm: [f64; 4],
}

impl ClimateCell {
    pub fn annual_temperature_c(&self) -> f64 {
        self.temperature_c.iter().sum::<f64>() * 0.25
    }
    pub fn annual_precipitation_mm(&self) -> f64 {
        self.precipitation_mm.iter().sum()
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MoistureBudget {
    pub steps: u32,
    pub timestep_s: f64,
    pub maximum_cfl: f64,
    pub evaporation_m3: f64,
    pub external_inflow_m3: f64,
    pub initial_storage_m3: f64,
    pub precipitation_m3: f64,
    pub external_outflow_m3: f64,
    pub final_storage_m3: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AtmosphereBoundary {
    pub cell: usize,
    /// 0 west, 1 east, 2 north (-Z), 3 south (+Z).
    pub face: u8,
    pub inflow_m3: [f64; 4],
    pub outflow_m3: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OceanEdge {
    pub from: usize,
    pub to: usize,
    /// Signed velocity from lower cell index to higher cell index.
    pub velocity_m_s: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OceanHeatBudget {
    pub initial_degree_m2: f64,
    pub relaxation_degree_m2: f64,
    pub final_degree_m2: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ClimatePlan {
    pub cells: Vec<ClimateCell>,
    pub moisture_budgets: [MoistureBudget; 4],
    pub atmosphere_boundary: Vec<AtmosphereBoundary>,
    pub ocean_edges: Vec<OceanEdge>,
    pub ocean_heat_budgets: [OceanHeatBudget; 4],
}

/// Canonical seeds are maximum depth×actual-area oceanic cells in each submerged
/// component (ties use row-major index). A component without oceanic crust stays
/// dry even at the domain boundary. Continental shelves connected to a seed flood.
pub fn ocean_mask(grid: &Grid, dem: &[f64], oceanic: &[bool]) -> Result<Vec<bool>, PrepareError> {
    ocean_mask_with_seeds(grid, dem, oceanic).map(|(mask, _)| mask)
}

/// Returns the exact canonical seeds and their flooded mask from one traversal.
/// Seed IDs are sorted row-major, independent of component traversal order.
pub fn ocean_mask_with_seeds(
    grid: &Grid,
    dem: &[f64],
    oceanic: &[bool],
) -> Result<(Vec<bool>, Vec<usize>), PrepareError> {
    validate_inputs(grid, dem, oceanic)?;
    let mut seen = vec![false; grid.len()];
    let mut mask = vec![false; grid.len()];
    let mut seeds = Vec::new();
    let mut queue = VecDeque::new();
    let mut component = Vec::new();
    for start in 0..grid.len() {
        if seen[start] || dem[start] >= 0.0 {
            continue;
        }
        component.clear();
        queue.push_back(start);
        seen[start] = true;
        let mut seed = None;
        let mut seed_score = -1.0;
        while let Some(i) = queue.pop_front() {
            component.push(i);
            let score = -dem[i] * grid.area(i);
            if !score.is_finite() {
                return Err(PrepareError::Invalid("ocean seed volume overflow"));
            }
            if oceanic[i]
                && (score > seed_score || (score == seed_score && seed.is_none_or(|s| i < s)))
            {
                seed = Some(i);
                seed_score = score;
            }
            for j in grid.neighbours(i) {
                if !seen[j] && dem[j] < 0.0 {
                    seen[j] = true;
                    queue.push_back(j);
                }
            }
        }
        if let Some(seed) = seed {
            seeds.push(seed);
            for &i in &component {
                mask[i] = true;
            }
        }
    }
    seeds.sort_unstable();
    Ok((mask, seeds))
}

pub fn solve(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    equator_z_m: f64,
    climate_radius_m: f64,
) -> Result<ClimatePlan, PrepareError> {
    solve_inner(grid, dem, ocean, equator_z_m, climate_radius_m, None, true)
}

fn validate_inputs(grid: &Grid, dem: &[f64], mask: &[bool]) -> Result<(), PrepareError> {
    validate_grid(grid)?;
    if dem.len() != grid.len() || mask.len() != grid.len() || !dem.iter().all(|v| v.is_finite()) {
        return Err(PrepareError::Invalid("invalid climate input arrays"));
    }
    Ok(())
}

fn validate_grid(grid: &Grid) -> Result<(), PrepareError> {
    grid.validate()?;
    for i in 0..grid.len() {
        let (x, z) = grid.center(i);
        if !x.is_finite() || !z.is_finite() || !grid.area(i).is_finite() || grid.area(i) <= 0.0 {
            return Err(PrepareError::Invalid("invalid climate cell geometry"));
        }
    }
    Ok(())
}

fn dimensions(grid: &Grid, i: usize) -> (f64, f64) {
    (
        grid.step_m
            .min(grid.width_m - (i % grid.columns) as f64 * grid.step_m),
        grid.step_m
            .min(grid.depth_m - (i / grid.columns) as f64 * grid.step_m),
    )
}

fn latitude(grid: &Grid, i: usize, equator: f64, radius: f64) -> f64 {
    ((equator - grid.center(i).1) / radius)
        .clamp(-std::f64::consts::FRAC_PI_2, std::f64::consts::FRAC_PI_2)
}

fn season_phase(season: usize) -> f64 {
    [0.0, 1.0, 0.0, -1.0][season]
}

fn equilibrium_sst(lat: f64, season: usize) -> f64 {
    28.0 - 43.0 * lat.sin().abs() + 3.0 * season_phase(season) * lat.sin()
}

/// A finite-volume face with signed flow from a to b (east or south).
#[derive(Clone, Copy)]
struct Face {
    a: usize,
    b: usize,
    flow_m2_s: f64,
}

fn faces(grid: &Grid, wind: &[[f64; 2]]) -> Vec<Face> {
    let mut result = Vec::with_capacity(grid.len() * 2);
    for i in 0..grid.len() {
        let (width, depth) = dimensions(grid, i);
        if i % grid.columns + 1 < grid.columns {
            result.push(Face {
                a: i,
                b: i + 1,
                flow_m2_s: 0.5 * (wind[i][0] + wind[i + 1][0]) * depth,
            });
        }
        if i / grid.columns + 1 < grid.rows {
            result.push(Face {
                a: i,
                b: i + grid.columns,
                flow_m2_s: -0.5 * (wind[i][1] + wind[i + grid.columns][1]) * width,
            });
        }
    }
    result
}

fn boundary_flow(grid: &Grid, wind: &[[f64; 2]], boundary: &AtmosphereBoundary) -> f64 {
    let (width, depth) = dimensions(grid, boundary.cell);
    let [east, north] = wind[boundary.cell];
    match boundary.face {
        0 => -east * depth,
        1 => east * depth,
        2 => north * width,
        3 => -north * width,
        _ => unreachable!("validated boundary face"),
    }
}

fn boundary_faces(grid: &Grid) -> Vec<AtmosphereBoundary> {
    let mut result = Vec::new();
    for i in 0..grid.len() {
        for face in 0..4 {
            if on_face(grid, i, face) {
                result.push(AtmosphereBoundary {
                    cell: i,
                    face,
                    inflow_m3: [0.0; 4],
                    outflow_m3: [0.0; 4],
                });
            }
        }
    }
    result
}

fn on_face(grid: &Grid, i: usize, face: u8) -> bool {
    match face {
        0 => i.is_multiple_of(grid.columns),
        1 => i % grid.columns + 1 == grid.columns,
        2 => i / grid.columns == 0,
        3 => i / grid.columns + 1 == grid.rows,
        _ => false,
    }
}

fn maximum_rate(grid: &Grid, faces: &[Face], boundaries: &[(usize, f64)], extra: &[f64]) -> f64 {
    let mut outgoing = extra.to_vec();
    for face in faces {
        outgoing[if face.flow_m2_s >= 0.0 {
            face.a
        } else {
            face.b
        }] += face.flow_m2_s.abs();
    }
    for &(i, flow) in boundaries {
        outgoing[i] += flow.max(0.0);
    }
    outgoing
        .iter()
        .enumerate()
        .map(|(i, flow)| flow / grid.area(i))
        .fold(0.0, f64::max)
}

fn ocean_edges(grid: &Grid, ocean: &[bool], eq: f64, radius: f64) -> Vec<OceanEdge> {
    let mut basin = vec![usize::MAX; grid.len()];
    let mut bounds = Vec::new();
    let mut queue = VecDeque::new();
    for start in 0..grid.len() {
        if !ocean[start] || basin[start] != usize::MAX {
            continue;
        }
        let id = bounds.len();
        let mut extent = [
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        ];
        basin[start] = id;
        queue.push_back(start);
        while let Some(i) = queue.pop_front() {
            let (x, z) = grid.center(i);
            extent[0] = extent[0].min(x);
            extent[1] = extent[1].max(x);
            extent[2] = extent[2].min(z);
            extent[3] = extent[3].max(z);
            for j in grid.neighbours(i) {
                if ocean[j] && basin[j] == usize::MAX {
                    basin[j] = id;
                    queue.push_back(j);
                }
            }
        }
        bounds.push(extent);
    }
    let mut east_flow = vec![0.0; grid.len()];
    let mut south_flow = vec![0.0; grid.len()];
    // Each all-ocean plaquette contributes a closed circulation. Their sum is
    // exactly divergence-free even around islands and on partial outer cells.
    // This is a discrete streamfunction: no current crosses an impermeable face.
    for i in 0..grid.len() {
        if i % grid.columns + 1 >= grid.columns || i / grid.columns + 1 >= grid.rows {
            continue;
        }
        let east = i + 1;
        let south = i + grid.columns;
        let southeast = south + 1;
        if ![i, east, south, southeast].into_iter().all(|j| ocean[j]) {
            continue;
        }
        let [min_x, max_x, min_z, max_z] = bounds[basin[i]];
        let (x0, z0) = grid.center(i);
        let (x1, z1) = grid.center(southeast);
        let x = (x0 + x1) * 0.5;
        let z = (z0 + z1) * 0.5;
        let sx = (max_x - min_x).max(grid.step_m);
        let sz = (max_z - min_z).max(grid.step_m);
        let hemisphere = if (min_z + max_z) * 0.5 <= eq {
            1.0
        } else {
            -1.0
        };
        let zonal = 0.8 + 0.2 * (3.0 * ((eq - z) / radius)).cos();
        let psi = hemisphere
            * zonal
            * 0.2
            * sx.min(sz)
            * (std::f64::consts::PI * (x - min_x) / sx).sin()
            * (std::f64::consts::PI * (z - min_z) / sz).sin();
        east_flow[i] += psi;
        south_flow[east] += psi;
        east_flow[south] -= psi;
        south_flow[i] -= psi;
    }
    let mut result = Vec::new();
    let mut maximum_speed: f64 = 0.0;
    for i in 0..grid.len() {
        if !ocean[i] {
            continue;
        }
        let (w, d) = dimensions(grid, i);
        for j in grid.neighbours(i).filter(|&j| j > i && ocean[j]) {
            let east = j == i + 1 && i % grid.columns + 1 < grid.columns;
            let speed = if east {
                east_flow[i] / d
            } else {
                south_flow[i] / w
            };
            maximum_speed = maximum_speed.max(speed.abs());
            result.push(OceanEdge {
                from: i,
                to: j,
                velocity_m_s: speed,
            });
        }
    }
    if maximum_speed > 0.6 {
        for edge in &mut result {
            edge.velocity_m_s *= 0.6 / maximum_speed;
        }
    }
    result
}

fn ocean_temperature(
    grid: &Grid,
    ocean: &[bool],
    equilibrium: &[f64],
    edges: &[OceanEdge],
    enabled: bool,
) -> (Vec<f64>, OceanHeatBudget) {
    let mut temperature = equilibrium.to_vec();
    let initial = (0..grid.len())
        .filter(|&i| ocean[i])
        .map(|i| equilibrium[i] * grid.area(i))
        .sum();
    let mut relaxation = 0.0;
    if enabled && !edges.is_empty() {
        let heat_faces: Vec<_> = edges
            .iter()
            .map(|edge| {
                let (w, d) = dimensions(grid, edge.from);
                Face {
                    a: edge.from,
                    b: edge.to,
                    flow_m2_s: edge.velocity_m_s
                        * if edge.to == edge.from + 1 && edge.from % grid.columns + 1 < grid.columns
                        {
                            d
                        } else {
                            w
                        },
                }
            })
            .collect();
        // Diffusion uses geometric face conductance, and its outgoing rate is
        // included in the positivity/CFL bound along with advection.
        let mut conductance = Vec::with_capacity(edges.len());
        let mut diffusion_out = vec![0.0; grid.len()];
        for edge in edges {
            let (ax, az) = grid.center(edge.from);
            let (bx, bz) = grid.center(edge.to);
            let (w, d) = dimensions(grid, edge.from);
            let length = if bx != ax { d } else { w };
            let k = 0.025 * grid.step_m * length / (bx - ax).hypot(bz - az);
            conductance.push(k);
            diffusion_out[edge.from] += k;
            diffusion_out[edge.to] += k;
        }
        let rate = maximum_rate(grid, &heat_faces, &[], &diffusion_out);
        let dt = CFL / rate;
        let mut energy = vec![0.0; grid.len()];
        for _ in 0..STEPS {
            for i in 0..grid.len() {
                energy[i] = temperature[i] * grid.area(i);
            }
            for (face, &k) in heat_faces.iter().zip(&conductance) {
                let donor = if face.flow_m2_s >= 0.0 {
                    face.a
                } else {
                    face.b
                };
                let transfer = dt
                    * (face.flow_m2_s * temperature[donor]
                        + k * (temperature[face.a] - temperature[face.b]));
                energy[face.a] -= transfer;
                energy[face.b] += transfer;
            }
            for i in 0..grid.len() {
                if ocean[i] {
                    let advected = energy[i] / grid.area(i);
                    let correction = 0.015 * (equilibrium[i] - advected);
                    temperature[i] = advected + correction;
                    relaxation += correction * grid.area(i);
                }
            }
        }
    }
    for i in 0..grid.len() {
        if !ocean[i] {
            temperature[i] = 0.0;
        }
    }
    let final_heat = temperature
        .iter()
        .enumerate()
        .map(|(i, t)| t * grid.area(i))
        .sum();
    (
        temperature,
        OceanHeatBudget {
            initial_degree_m2: initial,
            relaxation_degree_m2: relaxation,
            final_degree_m2: final_heat,
        },
    )
}

fn maritime_influence(grid: &Grid, ocean: &[bool]) -> (Vec<f64>, Vec<usize>) {
    let mut distance = vec![f64::INFINITY; grid.len()];
    let mut source = vec![usize::MAX; grid.len()];
    let mut queue = VecDeque::new();
    for i in 0..grid.len() {
        if ocean[i] {
            distance[i] = 0.0;
            source[i] = i;
            queue.push_back(i);
        }
    }
    // Uniform topological hop distance; the final fetch scale is in model metres.
    while let Some(i) = queue.pop_front() {
        for j in grid.neighbours(i) {
            if source[j] == usize::MAX {
                distance[j] = distance[i] + grid.step_m;
                source[j] = source[i];
                queue.push_back(j);
            }
        }
    }
    (distance, source)
}

fn solve_inner(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    equator: f64,
    radius: f64,
    custom_wind: Option<[f64; 2]>,
    heat_enabled: bool,
) -> Result<ClimatePlan, PrepareError> {
    validate_inputs(grid, dem, ocean)?;
    if !equator.is_finite()
        || !radius.is_finite()
        || radius <= 0.0
        || custom_wind.is_some_and(|v| !v.into_iter().all(f64::is_finite))
    {
        return Err(PrepareError::Invalid("invalid climate physical parameters"));
    }
    let n = grid.len();
    // Atmospheric relief is the water surface over oceans, not their bathymetry.
    // A disconnected dry basin below the sea datum keeps its physical height.
    let surface_height = |i: usize| if ocean[i] { 0.0 } else { dem[i] };
    let mut cells: Vec<_> = ocean
        .iter()
        .map(|&ocean| ClimateCell {
            temperature_c: [0.0; 4],
            precipitation_mm: [0.0; 4],
            potential_et_mm: [0.0; 4],
            snow_fraction: [0.0; 4],
            wind_m_s: [[0.0; 2]; 4],
            ocean,
            sst_c: [0.0; 4],
            evaporation_mm: [0.0; 4],
            final_vapor_mm: [0.0; 4],
            final_condensate_mm: [0.0; 4],
        })
        .collect();
    let edges = ocean_edges(grid, ocean, equator, radius);
    let (distance, source) = maritime_influence(grid, ocean);
    let mut boundary = boundary_faces(grid);
    let mut budgets = Vec::with_capacity(4);
    let mut heat_budgets = Vec::with_capacity(4);
    let zero = vec![0.0; n];
    let mut vapor = vec![0.0; n];
    let mut cloud = vec![0.0; n];
    let mut next_vapor = vec![0.0; n];
    let mut next_cloud = vec![0.0; n];
    let mut uplift = vec![0.0; n];
    let mut inflow_weight = vec![0.0; n];
    let mut rain = vec![0.0; n];
    for season in 0..4 {
        let phase = season_phase(season);
        let equilibrium: Vec<_> = (0..n)
            .map(|i| equilibrium_sst(latitude(grid, i, equator, radius), season))
            .collect();
        let (sst, heat_budget) = ocean_temperature(grid, ocean, &equilibrium, &edges, heat_enabled);
        heat_budgets.push(heat_budget);
        let mut wind = Vec::with_capacity(n);
        for i in 0..n {
            let lat = latitude(grid, i, equator, radius);
            let maritime = (-distance[i] / 350_000.0).exp();
            let local_base = 28.0 - 48.0 * lat.sin().abs();
            let amplitude = (2.0 + 20.0 * lat.sin().abs()) * (1.0 - 0.75 * maritime);
            let ocean_anomaly = if source[i] == usize::MAX {
                0.0
            } else {
                sst[source[i]] - equilibrium[source[i]]
            };
            let temperature = local_base + phase * lat.sin().signum() * amplitude
                - LAPSE_C_PER_M * surface_height(i)
                + maritime * ocean_anomaly;
            cells[i].temperature_c[season] = temperature;
            cells[i].sst_c[season] = sst[i];
            let pet =
                (45.0 + 7.0 * (temperature + 5.0).max(0.0)) * (1.0 + 0.15 * phase * lat.sin());
            cells[i].potential_et_mm[season] = pet;
            cells[i].snow_fraction[season] = ((2.0 - temperature) / 5.0).clamp(0.0, 1.0);
            // Smooth trade/westerly/polar bands; seasonally migrating convergence
            // and a continental thermal deflection, in east/north basis.
            let shifted = lat - phase * 0.10;
            let continental = 1.0 - maritime;
            let velocity = custom_wind.unwrap_or([
                -7.0 * (3.0 * shifted).cos(),
                -2.5 * (2.0 * shifted).sin() + phase * continental * 1.5,
            ]);
            wind.push(velocity);
            cells[i].wind_m_s[season] = velocity;
            cells[i].evaporation_mm[season] = if ocean[i] {
                (65.0 + 6.5 * (sst[i] + 5.0).max(0.0)).min(500.0)
            } else {
                0.0
            };
        }
        let transport = faces(grid, &wind);
        let boundary_rates: Vec<_> = boundary
            .iter()
            .map(|b| (b.cell, boundary_flow(grid, &wind, b)))
            .collect();
        let rate = maximum_rate(grid, &transport, &boundary_rates, &zero);
        let dt = if rate > 0.0 { CFL / rate } else { 1.0 };
        vapor.fill(0.0);
        cloud.fill(0.0);
        rain.fill(0.0);
        uplift.fill(0.0);
        inflow_weight.fill(0.0);
        for (i, stored) in vapor.iter_mut().enumerate() {
            *stored = INITIAL_VAPOR_M * grid.area(i);
        }
        for face in &transport {
            let (donor, recipient) = if face.flow_m2_s >= 0.0 {
                (face.a, face.b)
            } else {
                (face.b, face.a)
            };
            uplift[recipient] +=
                (surface_height(recipient) - surface_height(donor)) * face.flow_m2_s.abs();
            inflow_weight[recipient] += face.flow_m2_s.abs();
        }
        for i in 0..n {
            if inflow_weight[i] > 0.0 {
                uplift[i] /= inflow_weight[i];
            }
        }
        for _ in 0..STEPS {
            for i in 0..n {
                vapor[i] +=
                    cells[i].evaporation_mm[season] * 0.001 * grid.area(i) / f64::from(STEPS);
            }
            next_vapor.copy_from_slice(&vapor);
            next_cloud.copy_from_slice(&cloud);
            for face in &transport {
                let (donor, recipient) = if face.flow_m2_s >= 0.0 {
                    (face.a, face.b)
                } else {
                    (face.b, face.a)
                };
                let fraction = dt * face.flow_m2_s.abs() / grid.area(donor);
                let v = vapor[donor] * fraction;
                let c = cloud[donor] * fraction;
                next_vapor[donor] -= v;
                next_vapor[recipient] += v;
                next_cloud[donor] -= c;
                next_cloud[recipient] += c;
            }
            for (b, &(_, flow)) in boundary.iter_mut().zip(&boundary_rates) {
                if flow < 0.0 {
                    let input = -dt * flow * EXTERNAL_VAPOR_M;
                    next_vapor[b.cell] += input;
                    b.inflow_m3[season] += input;
                } else {
                    let fraction = dt * flow / grid.area(b.cell);
                    let v = vapor[b.cell] * fraction;
                    let c = cloud[b.cell] * fraction;
                    next_vapor[b.cell] -= v;
                    next_cloud[b.cell] -= c;
                    b.outflow_m3[season] += v + c;
                }
            }
            std::mem::swap(&mut vapor, &mut next_vapor);
            std::mem::swap(&mut cloud, &mut next_cloud);
            for i in 0..n {
                // Condensate travels in subsequent advection steps; fallout is
                // finite, so the rain maximum need not coincide with the crest.
                let fallout = cloud[i] * 0.35;
                cloud[i] -= fallout;
                rain[i] += fallout;
                let fraction = if uplift[i] >= 0.0 {
                    0.012 + 0.72 * (1.0 - (-uplift[i] / 650.0).exp())
                } else {
                    0.003
                };
                let capacity = 0.018
                    * (cells[i].temperature_c[season] / 25.0)
                        .clamp(-8.0, 4.0)
                        .exp()
                    * grid.area(i);
                let condensation = (vapor[i] * fraction).max((vapor[i] - capacity).max(0.0));
                vapor[i] -= condensation;
                cloud[i] += condensation;
            }
        }
        let mut evaporation = 0.0;
        let mut precipitation = 0.0;
        let mut storage = 0.0;
        for i in 0..n {
            cells[i].precipitation_mm[season] = rain[i] / grid.area(i) * 1000.0;
            cells[i].final_vapor_mm[season] = vapor[i] / grid.area(i) * 1000.0;
            cells[i].final_condensate_mm[season] = cloud[i] / grid.area(i) * 1000.0;
            evaporation += cells[i].evaporation_mm[season] * 0.001 * grid.area(i);
            precipitation += rain[i];
            storage += vapor[i] + cloud[i];
        }
        budgets.push(MoistureBudget {
            steps: STEPS,
            timestep_s: dt,
            maximum_cfl: dt * rate,
            evaporation_m3: evaporation,
            external_inflow_m3: boundary.iter().map(|b| b.inflow_m3[season]).sum(),
            initial_storage_m3: (0..n).map(|i| INITIAL_VAPOR_M * grid.area(i)).sum(),
            precipitation_m3: precipitation,
            external_outflow_m3: boundary.iter().map(|b| b.outflow_m3[season]).sum(),
            final_storage_m3: storage,
        });
    }
    let plan = ClimatePlan {
        cells,
        moisture_budgets: budgets.try_into().expect("four seasons"),
        atmosphere_boundary: boundary,
        ocean_edges: edges,
        ocean_heat_budgets: heat_budgets.try_into().expect("four seasons"),
    };
    plan.validate(grid)?;
    Ok(plan)
}

fn balanced(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
}

impl ClimatePlan {
    pub fn validate(&self, grid: &Grid) -> Result<(), PrepareError> {
        validate_grid(grid)?;
        if self.cells.len() != grid.len() {
            return Err(PrepareError::Invalid("climate cell count mismatch"));
        }
        for cell in &self.cells {
            if !cell
                .temperature_c
                .iter()
                .chain(&cell.sst_c)
                .all(|v| v.is_finite())
                || !cell.wind_m_s.iter().flatten().all(|v| v.is_finite())
                || !cell
                    .precipitation_mm
                    .iter()
                    .chain(&cell.potential_et_mm)
                    .chain(&cell.evaporation_mm)
                    .chain(&cell.final_vapor_mm)
                    .chain(&cell.final_condensate_mm)
                    .all(|v| v.is_finite() && *v >= 0.0)
                || !cell
                    .snow_fraction
                    .iter()
                    .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
                || (!cell.ocean && (cell.sst_c != [0.0; 4] || cell.evaporation_mm != [0.0; 4]))
            {
                return Err(PrepareError::Invalid("invalid persisted climate cell"));
            }
        }
        let mut previous = None;
        let mut divergence = vec![0.0; grid.len()];
        let mut throughput = vec![0.0; grid.len()];
        for edge in &self.ocean_edges {
            if edge.from >= grid.len()
                || edge.to >= grid.len()
                || edge.from >= edge.to
                || !self.cells[edge.from].ocean
                || !self.cells[edge.to].ocean
                || !grid.neighbours(edge.from).any(|j| j == edge.to)
                || !edge.velocity_m_s.is_finite()
                || edge.velocity_m_s.abs() > 0.6 + 1e-12
                || previous.is_some_and(|p| p >= (edge.from, edge.to))
            {
                return Err(PrepareError::Invalid("invalid ocean heat edge"));
            }
            previous = Some((edge.from, edge.to));
            let (w, d) = dimensions(grid, edge.from);
            let east = edge.to == edge.from + 1 && edge.from % grid.columns + 1 < grid.columns;
            let flux = edge.velocity_m_s * if east { d } else { w };
            divergence[edge.from] += flux;
            divergence[edge.to] -= flux;
            throughput[edge.from] += flux.abs();
            throughput[edge.to] += flux.abs();
        }
        if divergence
            .iter()
            .zip(&throughput)
            .any(|(d, q)| d.abs() > 1e-10 * q.max(1.0))
        {
            return Err(PrepareError::Invalid("unbalanced ocean circulation"));
        }
        let expected_edges = (0..grid.len())
            .filter(|&i| self.cells[i].ocean)
            .map(|i| {
                grid.neighbours(i)
                    .filter(|&j| j > i && self.cells[j].ocean)
                    .count()
            })
            .sum::<usize>();
        if expected_edges != self.ocean_edges.len() {
            return Err(PrepareError::Invalid("missing ocean heat edge"));
        }
        let expected_boundary = boundary_faces(grid);
        if self.atmosphere_boundary.len() != expected_boundary.len() {
            return Err(PrepareError::Invalid("missing atmospheric boundary"));
        }
        for (b, expected) in self.atmosphere_boundary.iter().zip(&expected_boundary) {
            if (b.cell, b.face) != (expected.cell, expected.face)
                || !b
                    .inflow_m3
                    .iter()
                    .chain(&b.outflow_m3)
                    .all(|v| v.is_finite() && *v >= 0.0)
            {
                return Err(PrepareError::Invalid("invalid atmospheric boundary"));
            }
        }
        let zero = vec![0.0; grid.len()];
        for season in 0..4 {
            let budget = &self.moisture_budgets[season];
            let quantities = [
                budget.evaporation_m3,
                budget.external_inflow_m3,
                budget.initial_storage_m3,
                budget.precipitation_m3,
                budget.external_outflow_m3,
                budget.final_storage_m3,
            ];
            if budget.steps != STEPS
                || !budget.timestep_s.is_finite()
                || budget.timestep_s <= 0.0
                || !budget.maximum_cfl.is_finite()
                || !(0.0..=CFL + 1e-12).contains(&budget.maximum_cfl)
                || !quantities.into_iter().all(|v| v.is_finite() && v >= 0.0)
            {
                return Err(PrepareError::Invalid("invalid atmospheric budget"));
            }
            let wind: Vec<_> = self.cells.iter().map(|c| c.wind_m_s[season]).collect();
            let transport = faces(grid, &wind);
            let rates: Vec<_> = self
                .atmosphere_boundary
                .iter()
                .map(|b| (b.cell, boundary_flow(grid, &wind, b)))
                .collect();
            let cfl = budget.timestep_s * maximum_rate(grid, &transport, &rates, &zero);
            if !balanced(cfl, budget.maximum_cfl) {
                return Err(PrepareError::Invalid("atmospheric CFL mismatch"));
            }
            let mut precip = 0.0;
            let mut evap = 0.0;
            let mut final_storage = 0.0;
            let mut initial_storage = 0.0;
            for (i, c) in self.cells.iter().enumerate() {
                let area = grid.area(i);
                precip += c.precipitation_mm[season] * 0.001 * area;
                evap += c.evaporation_mm[season] * 0.001 * area;
                final_storage +=
                    (c.final_vapor_mm[season] + c.final_condensate_mm[season]) * 0.001 * area;
                initial_storage += INITIAL_VAPOR_M * area;
            }
            let inflow = self
                .atmosphere_boundary
                .iter()
                .map(|b| b.inflow_m3[season])
                .sum();
            let outflow = self
                .atmosphere_boundary
                .iter()
                .map(|b| b.outflow_m3[season])
                .sum();
            if !balanced(precip, budget.precipitation_m3)
                || !balanced(evap, budget.evaporation_m3)
                || !balanced(final_storage, budget.final_storage_m3)
                || !balanced(initial_storage, budget.initial_storage_m3)
                || !balanced(inflow, budget.external_inflow_m3)
                || !balanced(outflow, budget.external_outflow_m3)
                || !balanced(
                    precip + outflow + final_storage,
                    evap + inflow + initial_storage,
                )
            {
                return Err(PrepareError::Invalid("atmospheric water budget mismatch"));
            }
            for (b, &(_, flow)) in self.atmosphere_boundary.iter().zip(&rates) {
                let expected_inflow =
                    (-flow).max(0.0) * budget.timestep_s * f64::from(STEPS) * EXTERNAL_VAPOR_M;
                if !balanced(expected_inflow, b.inflow_m3[season])
                    || (flow <= 0.0 && b.outflow_m3[season] != 0.0)
                {
                    return Err(PrepareError::Invalid("atmospheric boundary flux mismatch"));
                }
            }
            let heat = &self.ocean_heat_budgets[season];
            let heat_sum = self
                .cells
                .iter()
                .enumerate()
                .map(|(i, c)| c.sst_c[season] * grid.area(i))
                .sum();
            if !heat.initial_degree_m2.is_finite()
                || !heat.relaxation_degree_m2.is_finite()
                || !heat.final_degree_m2.is_finite()
                || !balanced(
                    heat.initial_degree_m2 + heat.relaxation_degree_m2,
                    heat.final_degree_m2,
                )
                || !balanced(heat_sum, heat.final_degree_m2)
            {
                return Err(PrepareError::Invalid("ocean heat budget mismatch"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "climate_tests.rs"]
mod tests;
