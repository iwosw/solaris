//! Deterministic finite-volume hydrology on the unmodified physical DEM.
//! Routing through a depression represents a collapsed reservoir, not an excavated channel.
use super::{grid::Grid, prepared::PrepareError};
use serde::{Deserialize, Serialize};
use std::cmp::Ordering;
use std::collections::{BTreeSet, BinaryHeap, VecDeque};

pub const SECONDS_PER_MODEL_YEAR: f64 = 365.0 * 86400.0;
pub const SECONDS_PER_SEASON: f64 = SECONDS_PER_MODEL_YEAR / 4.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunoffPlan {
    pub local_m3: Vec<[f64; 4]>,
    pub precipitation_m3: Vec<[f64; 4]>,
    pub actual_et_m3: Vec<[f64; 4]>,
    pub snowfall_m3: Vec<[f64; 4]>,
    pub snowmelt_m3: Vec<[f64; 4]>,
    pub infiltration_m3: Vec<[f64; 4]>,
    pub recharge_m3: Vec<[f64; 4]>,
    pub baseflow_m3: Vec<[f64; 4]>,
    /// End-of-season stocks; initial stocks are explicitly zero.
    pub snow_storage_m3: Vec<[f64; 4]>,
    pub soil_storage_m3: Vec<[f64; 4]>,
    pub groundwater_storage_m3: Vec<[f64; 4]>,
}

fn nonnegative(v: f64) -> bool {
    v.is_finite() && v >= 0.0
}
fn close(a: f64, b: f64) -> bool {
    (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
}
fn arrays(grid: &Grid, values: &[[f64; 4]]) -> bool {
    values.len() == grid.len() && values.iter().flatten().copied().all(nonnegative)
}

/// A single climatological year from dry initial stocks. Snow melts at 90 mm/°C
/// per season; 100 mm soil bucket recharges groundwater, which releases half its
/// stock each season. Infiltration/recharge are transfers, never hidden sinks.
/// Supplied ocean cells are external marine boundary reservoirs: all
/// precipitation water-equivalent exits to the ocean without terrestrial
/// snow/soil/groundwater stocks or ET. Sea evaporation belongs to climate.
pub fn runoff(
    grid: &Grid,
    temperature_c: &[[f64; 4]],
    precipitation_mm: &[[f64; 4]],
    potential_et_mm: &[[f64; 4]],
    snow_fraction: &[[f64; 4]],
    ocean: &[bool],
) -> Result<RunoffPlan, PrepareError> {
    grid.validate()?;
    if temperature_c.len() != grid.len()
        || ocean.len() != grid.len()
        || !temperature_c.iter().flatten().all(|v| v.is_finite())
        || !arrays(grid, precipitation_mm)
        || !arrays(grid, potential_et_mm)
        || !arrays(grid, snow_fraction)
        || snow_fraction.iter().flatten().any(|v| *v > 1.0)
    {
        return Err(PrepareError::Invalid("invalid seasonal runoff inputs"));
    }
    let zeros = || vec![[0.0; 4]; grid.len()];
    let mut p = RunoffPlan {
        local_m3: zeros(),
        precipitation_m3: zeros(),
        actual_et_m3: zeros(),
        snowfall_m3: zeros(),
        snowmelt_m3: zeros(),
        infiltration_m3: zeros(),
        recharge_m3: zeros(),
        baseflow_m3: zeros(),
        snow_storage_m3: zeros(),
        soil_storage_m3: zeros(),
        groundwater_storage_m3: zeros(),
    };
    for (i, &marine) in ocean.iter().enumerate() {
        let unit = grid.area(i) * 0.001;
        if marine {
            p.precipitation_m3[i] = precipitation_mm[i].map(|v| v * unit);
            p.local_m3[i] = p.precipitation_m3[i];
            continue;
        }
        let (mut snow, mut soil, mut groundwater) = (0.0_f64, 0.0_f64, 0.0_f64);
        for s in 0..4 {
            let rain = precipitation_mm[i][s] * unit;
            let snowfall = rain * snow_fraction[i][s];
            snow += snowfall;
            let melt = snow.min(temperature_c[i][s].max(0.0) * 90.0 * unit);
            snow -= melt;
            let liquid = rain - snowfall + melt;
            let infiltration = liquid * 0.6;
            soil += infiltration;
            let demand = potential_et_mm[i][s] * unit;
            let surface_et = (liquid - infiltration).min(demand);
            let soil_et = soil.min(demand - surface_et);
            soil -= soil_et;
            let recharge = (soil - 100.0 * unit).max(0.0);
            soil -= recharge;
            groundwater += recharge;
            let baseflow = groundwater * 0.5;
            groundwater -= baseflow;
            p.local_m3[i][s] = liquid - infiltration - surface_et + baseflow;
            p.precipitation_m3[i][s] = rain;
            p.actual_et_m3[i][s] = surface_et + soil_et;
            p.snowfall_m3[i][s] = snowfall;
            p.snowmelt_m3[i][s] = melt;
            p.infiltration_m3[i][s] = infiltration;
            p.recharge_m3[i][s] = recharge;
            p.baseflow_m3[i][s] = baseflow;
            p.snow_storage_m3[i][s] = snow;
            p.soil_storage_m3[i][s] = soil;
            p.groundwater_storage_m3[i][s] = groundwater;
        }
    }
    p.validate(grid, ocean)?;
    Ok(p)
}
impl RunoffPlan {
    pub fn validate(&self, grid: &Grid, ocean: &[bool]) -> Result<(), PrepareError> {
        grid.validate()?;
        if ocean.len() != grid.len() {
            return Err(PrepareError::Invalid("invalid runoff ocean mask length"));
        }
        for field in [
            &self.local_m3,
            &self.precipitation_m3,
            &self.actual_et_m3,
            &self.snowfall_m3,
            &self.snowmelt_m3,
            &self.infiltration_m3,
            &self.recharge_m3,
            &self.baseflow_m3,
            &self.snow_storage_m3,
            &self.soil_storage_m3,
            &self.groundwater_storage_m3,
        ] {
            if !arrays(grid, field) {
                return Err(PrepareError::Invalid("invalid runoff plan arrays"));
            }
        }
        for (i, &marine) in ocean.iter().enumerate() {
            if marine {
                if self.local_m3[i] != self.precipitation_m3[i]
                    || [
                        &self.actual_et_m3,
                        &self.snowfall_m3,
                        &self.snowmelt_m3,
                        &self.infiltration_m3,
                        &self.recharge_m3,
                        &self.baseflow_m3,
                        &self.snow_storage_m3,
                        &self.soil_storage_m3,
                        &self.groundwater_storage_m3,
                    ]
                    .into_iter()
                    .any(|field| field[i] != [0.0; 4])
                {
                    return Err(PrepareError::Invalid(
                        "marine runoff boundary water balance",
                    ));
                }
                continue;
            }
            let (mut snow, mut soil, mut ground) = (0.0, 0.0, 0.0);
            for s in 0..4 {
                let new_snow = self.snow_storage_m3[i][s];
                let new_soil = self.soil_storage_m3[i][s];
                let new_ground = self.groundwater_storage_m3[i][s];
                let liquid =
                    self.precipitation_m3[i][s] - self.snowfall_m3[i][s] + self.snowmelt_m3[i][s];
                let soil_et = soil + self.infiltration_m3[i][s] - self.recharge_m3[i][s] - new_soil;
                let surface_et = self.actual_et_m3[i][s] - soil_et;
                // ET is reconstructed by subtracting several large finite-volume
                // stocks. Bound cancellation by their f64 rounding scale, not an
                // absolute cubic-meter epsilon that rejects valid coarse cells.
                let et_roundoff = (8.0
                    * f64::EPSILON
                    * soil
                        .max(self.infiltration_m3[i][s])
                        .max(self.recharge_m3[i][s])
                        .max(new_soil)
                        .max(self.actual_et_m3[i][s]))
                .max(1e-8);
                let reconstructed_local =
                    liquid - self.infiltration_m3[i][s] - surface_et + self.baseflow_m3[i][s];
                if !close(
                    snow + self.snowfall_m3[i][s],
                    new_snow + self.snowmelt_m3[i][s],
                ) || !close(
                    ground + self.recharge_m3[i][s],
                    new_ground + self.baseflow_m3[i][s],
                ) || self.infiltration_m3[i][s] > liquid + 1e-8
                    || self.snowfall_m3[i][s] > self.precipitation_m3[i][s] + 1e-8
                    || soil_et < -et_roundoff
                    || surface_et < -et_roundoff
                    || surface_et > liquid - self.infiltration_m3[i][s] + et_roundoff
                    || (!close(self.local_m3[i][s], reconstructed_local)
                        && (self.local_m3[i][s] - reconstructed_local).abs() > et_roundoff)
                    || !close(
                        self.precipitation_m3[i][s] + snow + soil + ground,
                        self.actual_et_m3[i][s]
                            + self.local_m3[i][s]
                            + new_snow
                            + new_soil
                            + new_ground,
                    )
                {
                    return Err(PrepareError::Invalid("seasonal runoff water balance"));
                }
                (snow, soil, ground) = (new_snow, new_soil, new_ground);
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Depression {
    pub id: usize,
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    /// Exclusively owned marginal cells. Full footprint is this list plus
    /// descendants, never a quadratic set of duplicated parent footprints.
    pub cells: Vec<usize>,
    pub minimum_m: f64,
    pub spill_level_m: f64,
    pub spill_from: usize,
    pub spill_to: usize,
    pub capacity_m3: f64,
    pub storage_m3: [f64; 4],
    pub level_m: [Option<f64>; 4],
    pub inflow_m3: [f64; 4],
    pub outflow_m3: [f64; 4],
    pub evaporation_m3: [f64; 4],
    pub groundwater_export_m3: [f64; 4],
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HydrologyPlan {
    /// Potential spill routing inside lake components; these edges are not river excavation.
    pub downstream: Vec<Option<usize>>,
    /// Per-cell canonical rank; downstream has smaller rank. It is NOT elevation.
    pub rank: Vec<usize>,
    /// Outgoing seasonal volumes, including conceptual transport inside collapsed lake nodes.
    pub discharge_m3: Vec<[f64; 4]>,
    /// Final-season physical water surface; None means dry, not sea-level water.
    pub water_level_m: Vec<Option<f64>>,
    pub seasonal_water_level_m: Vec<[Option<f64>; 4]>,
    pub depression: Vec<Option<usize>>,
    pub depressions: Vec<Depression>,
    pub ocean_outlets: Vec<usize>,
    pub external_outlets: Vec<usize>,
    /// Actual evaporation and groundwater export at losing reaches. Requested
    /// losses are capped by available water. Groundwater export crosses this model's boundary.
    pub evaporation_m3: Vec<[f64; 4]>,
    pub groundwater_export_m3: Vec<[f64; 4]>,
    /// Actual losses from lake stocks, owned by the collapsed reservoir rather than a reach.
    pub lake_evaporation_m3: Vec<[f64; 4]>,
    pub lake_groundwater_export_m3: Vec<[f64; 4]>,
    pub external_outflow_m3: [f64; 4],
    pub ocean_outflow_m3: [f64; 4],
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChannelLosses {
    pub evaporation_m3: Vec<[f64; 4]>,
    pub groundwater_export_m3: Vec<[f64; 4]>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LakeLosses {
    pub evaporation_demand_m3: Vec<[f64; 4]>,
    pub groundwater_export_demand_m3: Vec<[f64; 4]>,
}

#[derive(Clone, Copy)]
struct Flood {
    height: f64,
    cell: usize,
}
impl PartialEq for Flood {
    fn eq(&self, b: &Self) -> bool {
        self.height == b.height && self.cell == b.cell
    }
}
impl Eq for Flood {}
impl PartialOrd for Flood {
    fn partial_cmp(&self, b: &Self) -> Option<Ordering> {
        Some(self.cmp(b))
    }
}
impl Ord for Flood {
    fn cmp(&self, b: &Self) -> Ordering {
        b.height
            .total_cmp(&self.height)
            .then_with(|| b.cell.cmp(&self.cell))
    }
}

fn find(parent: &mut [usize], mut i: usize) -> usize {
    while parent[i] != i {
        parent[i] = parent[parent[i]];
        i = parent[i];
    }
    i
}

/// Ascending DEM union constructs a merge tree. A saddle joining two closed
/// components creates a parent; contact with an open component fixes the spill.
/// Equal-height unions have zero capacity and are harmless, deterministic flats.
fn hierarchy(grid: &Grid, dem: &[f64], ocean: &[bool]) -> Vec<Depression> {
    let n = grid.len();
    let mut order: Vec<_> = (0..n).collect();
    order.sort_by(|a, b| dem[*a].total_cmp(&dem[*b]).then(a.cmp(b)));
    let mut uf: Vec<_> = (0..n).collect();
    let mut active = vec![false; n];
    let mut node = vec![None; n];
    let mut nodes: Vec<Depression> = Vec::new();
    for i in order {
        active[i] = true;
        if !ocean[i] && !grid.boundary(i) {
            let id = nodes.len();
            nodes.push(Depression {
                id,
                parent: None,
                children: Vec::new(),
                cells: vec![i],
                minimum_m: dem[i],
                spill_level_m: dem[i],
                spill_from: i,
                spill_to: i,
                capacity_m3: 0.0,
                storage_m3: [0.0; 4],
                level_m: [None; 4],
                inflow_m3: [0.0; 4],
                outflow_m3: [0.0; 4],
                evaporation_m3: [0.0; 4],
                groundwater_export_m3: [0.0; 4],
            });
            node[i] = Some(id);
        }
        for j in grid.neighbours(i) {
            if !active[j] {
                continue;
            }
            let a = find(&mut uf, i);
            let b = find(&mut uf, j);
            if a == b {
                continue;
            }
            let merged = match (node[a], node[b]) {
                (Some(x), Some(y)) => {
                    // A newly activated, zero-depth cell merely extends its neighbour.
                    if nodes[x].cells.len() == 1
                        && nodes[x].children.is_empty()
                        && nodes[x].minimum_m == dem[i]
                    {
                        let absorbed = std::mem::take(&mut nodes[x].cells);
                        nodes[y].cells.extend(absorbed);
                        nodes[x].parent = Some(y);
                        Some(y)
                    } else {
                        let id = nodes.len();
                        for child in [x, y] {
                            nodes[child].parent = Some(id);
                            nodes[child].spill_level_m = dem[i];
                            nodes[child].spill_from = if child == x { i } else { j };
                            nodes[child].spill_to = if child == x { j } else { i };
                        }
                        nodes.push(Depression {
                            id,
                            parent: None,
                            children: vec![x, y],
                            minimum_m: nodes[x].minimum_m.min(nodes[y].minimum_m),
                            cells: Vec::new(),
                            spill_level_m: dem[i],
                            spill_from: i,
                            spill_to: j,
                            capacity_m3: 0.0,
                            storage_m3: [0.0; 4],
                            level_m: [None; 4],
                            inflow_m3: [0.0; 4],
                            outflow_m3: [0.0; 4],
                            evaporation_m3: [0.0; 4],
                            groundwater_export_m3: [0.0; 4],
                        });
                        Some(id)
                    }
                }
                (Some(x), None) => {
                    nodes[x].spill_level_m = dem[i];
                    nodes[x].spill_from = i;
                    nodes[x].spill_to = j;
                    None
                }
                (None, Some(y)) => {
                    nodes[y].spill_level_m = dem[i];
                    nodes[y].spill_from = j;
                    nodes[y].spill_to = i;
                    None
                }
                (None, None) => None,
            };
            uf[b] = a;
            node[a] = merged;
        }
    }
    // Remove absorbed activation nodes, preserving only actual minima and merges.
    let keep: Vec<_> = nodes
        .iter()
        .map(|d| d.parent.is_none_or(|p| nodes[p].children.contains(&d.id)))
        .collect();
    let mut map = vec![0; nodes.len()];
    let mut count = 0;
    for i in 0..nodes.len() {
        if keep[i] {
            map[i] = count;
            count += 1;
        }
    }
    let mut result = Vec::with_capacity(count);
    for d in nodes.iter().filter(|d| keep[d.id]) {
        let mut d = d.clone();
        d.id = map[d.id];
        d.parent = d.parent.map(|p| map[p]);
        d.children = d.children.into_iter().map(|c| map[c]).collect();
        d.cells.sort_unstable();
        d.cells.dedup();
        result.push(d);
    }
    let mut area = vec![0.0; result.len()];
    for i in 0..result.len() {
        area[i] = result[i].cells.iter().map(|c| grid.area(*c)).sum::<f64>()
            + result[i].children.iter().map(|c| area[*c]).sum::<f64>();
        result[i].capacity_m3 = result[i]
            .cells
            .iter()
            .map(|c| (result[i].spill_level_m - dem[*c]).max(0.0) * grid.area(*c))
            .sum::<f64>()
            + result[i]
                .children
                .iter()
                .map(|c| {
                    result[*c].capacity_m3
                        + (result[i].spill_level_m - result[*c].spill_level_m) * area[*c]
                })
                .sum::<f64>();
    }
    result
}

fn footprint(id: usize, nodes: &[Depression]) -> Vec<usize> {
    let mut result = Vec::new();
    let mut work = vec![id];
    while let Some(i) = work.pop() {
        result.extend_from_slice(&nodes[i].cells);
        work.extend_from_slice(&nodes[i].children);
    }
    result.sort_unstable();
    result
}

/// Invert the proved monotone, piecewise-linear finite-volume hypsometry.
/// Exact interval traversal; no convergence tolerance, epsilon slope or bisection.
fn level(grid: &Grid, dem: &[f64], cells: &[usize], volume: f64) -> Option<f64> {
    if volume <= 0.0 {
        return None;
    }
    let mut order = cells.to_vec();
    order.sort_by(|a, b| dem[*a].total_cmp(&dem[*b]).then(a.cmp(b)));
    let mut area = 0.0;
    let mut remaining = volume;
    let mut previous = dem[order[0]];
    for i in order {
        let interval = (dem[i] - previous) * area;
        if area > 0.0 && remaining <= interval {
            return Some(previous + remaining / area);
        }
        remaining -= interval;
        previous = dem[i];
        area += grid.area(i);
    }
    Some(previous + remaining / area)
}

/// Fill child reservoirs first. Overflow from one child fills the other, then
/// the shared parent rises from the common saddle. Storage is inclusive: parent
/// values contain children and are counted only at hierarchy roots in balances.
fn fill(
    id: usize,
    input: f64,
    season: usize,
    nodes: &mut [Depression],
    grid: &Grid,
    dem: &[f64],
) -> f64 {
    let children = nodes[id].children.clone();
    let prior = if season == 0 {
        0.0
    } else {
        nodes[id].storage_m3[season - 1]
    };
    let capacity = nodes[id].capacity_m3;
    let available = prior + input;
    nodes[id].inflow_m3[season] += input;
    let stored = available.min(capacity);
    nodes[id].storage_m3[season] = stored;
    nodes[id].outflow_m3[season] = available - stored;
    if children.is_empty() {
        nodes[id].level_m[season] = level(grid, dem, &nodes[id].cells, stored);
    } else {
        let child_capacity: f64 = children.iter().map(|c| nodes[*c].capacity_m3).sum();
        // Preserve last season's allocation below the saddle; incoming volumes
        // have already been assigned to leaf catchments by the routing pass.
        let mut assigned = 0.0;
        for &c in &children {
            assigned += nodes[c].storage_m3[season];
        }
        let mut extra = (stored.min(child_capacity) - assigned).max(0.0);
        for &c in &children {
            let take = extra.min(nodes[c].capacity_m3 - nodes[c].storage_m3[season]);
            if take > 0.0 {
                add_storage(c, take, season, nodes, grid, dem);
            }
            extra -= take;
        }
        if stored >= child_capacity && stored > 0.0 {
            let surface = level(grid, dem, &footprint(id, nodes), stored);
            set_surface(id, surface, season, nodes);
        }
    }
    available - stored
}
fn add_storage(
    id: usize,
    amount: f64,
    s: usize,
    nodes: &mut [Depression],
    grid: &Grid,
    dem: &[f64],
) {
    let mut work = vec![(id, amount)];
    while let Some((i, amount)) = work.pop() {
        nodes[i].storage_m3[s] += amount;
        nodes[i].inflow_m3[s] += amount;
        let mut remaining = amount;
        for k in 0..nodes[i].children.len() {
            let child = nodes[i].children[k];
            let take = remaining.min(nodes[child].capacity_m3 - nodes[child].storage_m3[s]);
            if take > 0.0 {
                work.push((child, take));
            }
            remaining -= take;
        }
    }
    refresh_levels(id, s, nodes, grid, dem);
}
fn set_surface(id: usize, surface: Option<f64>, s: usize, nodes: &mut [Depression]) {
    let mut work = vec![id];
    while let Some(i) = work.pop() {
        nodes[i].level_m[s] = surface;
        work.extend_from_slice(&nodes[i].children);
    }
}

pub fn solve(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    local_m3: &[[f64; 4]],
) -> Result<HydrologyPlan, PrepareError> {
    let losses = ChannelLosses {
        evaporation_m3: vec![[0.0; 4]; grid.len()],
        groundwater_export_m3: vec![[0.0; 4]; grid.len()],
    };
    solve_with_losses(grid, dem, ocean, local_m3, &losses)
}

pub fn solve_with_losses(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    local_m3: &[[f64; 4]],
    losses: &ChannelLosses,
) -> Result<HydrologyPlan, PrepareError> {
    solve_with_all_losses(grid, dem, ocean, local_m3, Some(losses), None)
}

/// Residual climate PET may be supplied as demand volumes. Only a physically
/// wetted footprint can spend demand, and withdrawal is capped by reachable
/// inflow plus lake stock. One fixed fill/withdraw step per season is explicit.
pub fn solve_with_lake_losses(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    local_m3: &[[f64; 4]],
    losses: &LakeLosses,
) -> Result<HydrologyPlan, PrepareError> {
    solve_with_all_losses(grid, dem, ocean, local_m3, None, Some(losses))
}

pub fn solve_with_all_losses(
    grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
    local_m3: &[[f64; 4]],
    losses: Option<&ChannelLosses>,
    lake_losses: Option<&LakeLosses>,
) -> Result<HydrologyPlan, PrepareError> {
    grid.validate()?;
    let n = grid.len();
    if dem.len() != n
        || ocean.len() != n
        || !dem.iter().all(|v| v.is_finite())
        || !arrays(grid, local_m3)
        || losses.is_some_and(|l| {
            !arrays(grid, &l.evaporation_m3) || !arrays(grid, &l.groundwater_export_m3)
        })
        || lake_losses.is_some_and(|l| {
            !arrays(grid, &l.evaporation_demand_m3)
                || !arrays(grid, &l.groundwater_export_demand_m3)
        })
    {
        return Err(PrepareError::Invalid("invalid hydrology inputs"));
    }
    let mut downstream = vec![None; n];
    let mut visited = vec![false; n];
    let mut heap = BinaryHeap::new();
    let mut flood_height = vec![0.0; n];
    let mut flood_rank = vec![0; n];
    for i in 0..n {
        if ocean[i] || grid.boundary(i) {
            visited[i] = true;
            flood_height[i] = dem[i];
            heap.push(Flood {
                height: dem[i],
                cell: i,
            });
        }
    }
    let mut count = 0;
    while let Some(Flood { height, cell: i }) = heap.pop() {
        flood_rank[i] = count;
        count += 1;
        for j in grid.neighbours(i) {
            if visited[j] {
                continue;
            }
            visited[j] = true;
            flood_height[j] = height.max(dem[j]);
            downstream[j] = Some(i);
            heap.push(Flood {
                height: flood_height[j],
                cell: j,
            });
        }
    }
    // True physical steepest descent on open hills; flats retain a separate rank.
    let flood_downstream = downstream.clone();
    for i in 0..n {
        if ocean[i] {
            downstream[i] = None;
            continue;
        }
        let (x, z) = grid.center(i);
        let mut best = None;
        let mut slope = 0.0;
        for j in grid.neighbours(i) {
            if dem[j] >= dem[i]
                || flood_height[j] > flood_height[i]
                || (flood_height[j] == flood_height[i] && flood_rank[j] >= flood_rank[i])
            {
                continue;
            }
            let (u, v) = grid.center(j);
            let candidate = (dem[i] - dem[j]) / (x - u).hypot(z - v);
            if candidate > slope {
                slope = candidate;
                best = Some(j);
            }
        }
        if best.is_some() {
            downstream[i] = best;
        }
    }
    let mut depressions = hierarchy(grid, dem, ocean);
    let mut depression = vec![None; n];
    let roots: Vec<_> = depressions
        .iter()
        .filter(|d| d.parent.is_none() && d.capacity_m3 > 0.0)
        .map(|d| d.id)
        .collect();
    for &r in &roots {
        for i in footprint(r, &depressions) {
            depression[i] = Some(r);
        }
    }
    // A root reservoir has one authoritative outlet. Route its conceptual
    // internal transport as a tree; do not change a single physical DEM value.
    for &r in &roots {
        let from = *footprint(r, &depressions)
            .iter()
            .min_by_key(|i| flood_rank[**i])
            .ok_or(PrepareError::Invalid("empty depression footprint"))?;
        let to = flood_downstream[from].ok_or(PrepareError::Invalid(
            "depression has no physical spill edge",
        ))?;
        depressions[r].spill_from = from;
        depressions[r].spill_to = to;
        downstream[from] = Some(to);
        let mut reached = BTreeSet::new();
        reached.insert(from);
        let mut queue = VecDeque::from([from]);
        while let Some(i) = queue.pop_front() {
            for j in grid.neighbours(i) {
                if depression[j] == Some(r) && reached.insert(j) {
                    downstream[j] = Some(i);
                    queue.push_back(j);
                }
            }
        }
    }
    let rank = topo_rank(&downstream)?;
    let mut order: Vec<_> = (0..n).collect();
    order.sort_unstable_by_key(|i| std::cmp::Reverse(rank[*i]));
    let mut p = HydrologyPlan {
        downstream,
        rank,
        discharge_m3: vec![[0.0; 4]; n],
        water_level_m: vec![None; n],
        seasonal_water_level_m: vec![[None; 4]; n],
        depression,
        depressions,
        ocean_outlets: Vec::new(),
        external_outlets: Vec::new(),
        evaporation_m3: vec![[0.0; 4]; n],
        groundwater_export_m3: vec![[0.0; 4]; n],
        lake_evaporation_m3: vec![[0.0; 4]; n],
        lake_groundwater_export_m3: vec![[0.0; 4]; n],
        external_outflow_m3: [0.0; 4],
        ocean_outflow_m3: [0.0; 4],
    };
    for (i, &marine) in ocean.iter().enumerate() {
        if p.downstream[i].is_none() {
            if marine {
                p.ocean_outlets.push(i);
            } else {
                p.external_outlets.push(i);
            }
        }
    }
    // Leaf labels use physical downslope, with canonical index routing on flats.
    let mut leaf = vec![None; n];
    for d in &p.depressions {
        if d.children.is_empty() {
            for &i in &d.cells {
                leaf[i] = Some(d.id);
            }
        }
    }
    for &r in &roots {
        let mut cells = footprint(r, &p.depressions);
        cells.sort_by(|a, b| dem[*a].total_cmp(&dem[*b]).then(a.cmp(b)));
        for i in cells {
            if leaf[i].is_some() {
                continue;
            }
            leaf[i] = grid
                .neighbours(i)
                .filter(|j| p.depression[*j] == Some(r) && dem[*j] <= dem[i])
                .filter_map(|j| leaf[j].map(|l| (dem[j], j, l)))
                .min_by(|a, b| a.0.total_cmp(&b.0).then(a.1.cmp(&b.1)))
                .map(|v| v.2)
                .or_else(|| descendant_leaf(r, &p.depressions));
        }
    }
    let mut totals = vec![0.0; p.depressions.len()];
    for s in 0..4 {
        let mut accumulated: Vec<f64> = local_m3.iter().map(|v| v[s]).collect();
        let mut leaf_input = vec![0.0; p.depressions.len()];
        for &i in &order {
            let mut volume = accumulated[i];
            let evaporation = volume.min(losses.map_or(0.0, |l| l.evaporation_m3[i][s]));
            volume -= evaporation;
            let export = volume.min(losses.map_or(0.0, |l| l.groundwater_export_m3[i][s]));
            volume -= export;
            p.evaporation_m3[i][s] = evaporation;
            p.groundwater_export_m3[i][s] = export;
            // Allocate each source once when it enters a reservoir component.
            if let Some(l) = leaf[i] {
                let incoming = local_m3[i][s]
                    + grid
                        .neighbours(i)
                        .filter(|j| {
                            p.downstream[*j] == Some(i) && p.depression[*j] != p.depression[i]
                        })
                        .map(|j| p.discharge_m3[j][s])
                        .sum::<f64>();
                leaf_input[l] += incoming;
            }
            if let Some(r) = p.depression[i]
                && i == p.depressions[r].spill_from
            {
                // Losses anywhere in the collapsed reservoir have one owner.
                let root_loss: f64 = footprint(r, &p.depressions)
                    .iter()
                    .map(|c| p.evaporation_m3[*c][s] + p.groundwater_export_m3[*c][s])
                    .sum();
                let input: f64 = subtree_input(r, &p.depressions, &leaf_input);
                let scale = if input > 0.0 {
                    ((input - root_loss).max(0.0) / input).min(1.0)
                } else {
                    0.0
                };
                solve_tree(
                    r,
                    s,
                    &leaf_input,
                    scale,
                    &mut totals,
                    &mut p.depressions,
                    (grid, dem),
                );
                if let Some(demand) = lake_losses {
                    withdraw_lake(r, s, &leaf, demand, &mut p, grid, dem);
                }
                volume = p.depressions[r].outflow_m3[s];
            }
            p.discharge_m3[i][s] = volume;
            if let Some(j) = p.downstream[i] {
                accumulated[j] += volume;
            } else if ocean[i] {
                p.ocean_outflow_m3[s] += volume;
            } else {
                p.external_outflow_m3[s] += volume;
            }
        }
        for &r in &roots {
            let cells = footprint(r, &p.depressions);
            for i in cells {
                let surface = leaf[i].and_then(|l| p.depressions[l].level_m[s]);
                p.seasonal_water_level_m[i][s] = surface.filter(|h| *h > dem[i]);
            }
        }
    }
    for i in 0..n {
        p.water_level_m[i] = p.seasonal_water_level_m[i][3];
    }
    p.validate(grid, dem, ocean, local_m3)?;
    Ok(p)
}
fn descendant_leaf(id: usize, nodes: &[Depression]) -> Option<usize> {
    let mut i = id;
    while let Some(c) = nodes[i].children.first() {
        i = *c;
    }
    Some(i)
}
fn subtree_input(id: usize, nodes: &[Depression], input: &[f64]) -> f64 {
    let mut sum = 0.0;
    let mut work = vec![id];
    while let Some(i) = work.pop() {
        if nodes[i].children.is_empty() {
            sum += input[i];
        } else {
            work.extend_from_slice(&nodes[i].children);
        }
    }
    sum
}
fn solve_tree(
    id: usize,
    s: usize,
    input: &[f64],
    scale: f64,
    totals: &mut [f64],
    nodes: &mut [Depression],
    surface: (&Grid, &[f64]),
) {
    let (grid, dem) = surface;
    let mut order = vec![id];
    let mut cursor = 0;
    while cursor < order.len() {
        let i = order[cursor];
        order.extend_from_slice(&nodes[i].children);
        cursor += 1;
    }
    order.sort_unstable();
    for i in order {
        totals[i] = if nodes[i].children.is_empty() {
            input[i]
        } else {
            nodes[i].children.iter().map(|c| totals[*c]).sum()
        };
        fill(i, totals[i] * scale, s, nodes, grid, dem);
    }
}

fn withdraw_lake(
    root: usize,
    s: usize,
    leaf: &[Option<usize>],
    demand: &LakeLosses,
    plan: &mut HydrologyPlan,
    grid: &Grid,
    dem: &[f64],
) {
    // Demand uses the reachable footprint after arrivals, before withdrawal.
    // This is one explicitly ordered seasonal integration step, not equilibrium.
    let footprint: Vec<_> = footprint(root, &plan.depressions)
        .into_iter()
        .filter_map(|i| {
            leaf[i]
                .filter(|l| plan.depressions[*l].level_m[s].is_some_and(|h| h > dem[i]))
                .map(|l| (i, l))
        })
        .collect();
    for (i, l) in footprint {
        plan.lake_evaporation_m3[i][s] = withdraw(
            root,
            l,
            s,
            demand.evaporation_demand_m3[i][s],
            false,
            &mut plan.depressions,
        );
        plan.lake_groundwater_export_m3[i][s] = withdraw(
            root,
            l,
            s,
            demand.groundwater_export_demand_m3[i][s],
            true,
            &mut plan.depressions,
        );
    }
    refresh_levels(root, s, &mut plan.depressions, grid, dem);
}

fn book_loss(d: &mut Depression, s: usize, amount: f64, groundwater: bool) {
    if groundwater {
        d.groundwater_export_m3[s] += amount;
    } else {
        d.evaporation_m3[s] += amount;
    }
}

fn withdraw(
    root: usize,
    leaf: usize,
    s: usize,
    demand: f64,
    groundwater: bool,
    nodes: &mut [Depression],
) -> f64 {
    let mut remaining = demand;
    // Pending spill is still reachable water during the seasonal loss step.
    let spill = remaining.min(nodes[root].outflow_m3[s]);
    nodes[root].outflow_m3[s] -= spill;
    book_loss(&mut nodes[root], s, spill, groundwater);
    remaining -= spill;
    let mut path = vec![leaf];
    while let Some(p) = nodes[*path.last().unwrap()].parent {
        path.push(p);
    }
    // Shared stock above a saddle is accessible first; once drained, each
    // separate leaf can lose only its own reachable stock, never its sibling's.
    for k in (0..path.len()).rev() {
        let id = path[k];
        let child_capacity: f64 = nodes[id]
            .children
            .iter()
            .map(|c| nodes[*c].capacity_m3)
            .sum();
        let available = (nodes[id].storage_m3[s] - child_capacity).max(0.0);
        let amount = remaining.min(available);
        for &ancestor in &path[k..] {
            nodes[ancestor].storage_m3[s] = (nodes[ancestor].storage_m3[s] - amount).max(0.0);
            book_loss(&mut nodes[ancestor], s, amount, groundwater);
        }
        remaining -= amount;
    }
    demand - remaining
}

fn refresh_levels(root: usize, s: usize, nodes: &mut [Depression], grid: &Grid, dem: &[f64]) {
    let mut ids = Vec::new();
    let mut work = vec![root];
    while let Some(i) = work.pop() {
        ids.push(i);
        work.extend_from_slice(&nodes[i].children);
    }
    for &i in &ids {
        let child_capacity: f64 = nodes[i]
            .children
            .iter()
            .map(|c| nodes[*c].capacity_m3)
            .sum();
        nodes[i].level_m[s] =
            if nodes[i].children.is_empty() || nodes[i].storage_m3[s] >= child_capacity {
                level(grid, dem, &footprint(i, nodes), nodes[i].storage_m3[s])
            } else {
                None
            };
    }
    for i in ids {
        if let Some(surface) = nodes[i].level_m[s] {
            for k in 0..nodes[i].children.len() {
                let child = nodes[i].children[k];
                nodes[child].level_m[s] = Some(surface);
            }
        }
    }
}
fn topo_rank(downstream: &[Option<usize>]) -> Result<Vec<usize>, PrepareError> {
    let n = downstream.len();
    let mut upstream = vec![Vec::new(); n];
    let mut ready = BTreeSet::new();
    for (i, d) in downstream.iter().enumerate() {
        if let Some(j) = d {
            if *j >= n {
                return Err(PrepareError::Invalid("unknown downstream cell"));
            }
            upstream[*j].push(i);
        } else {
            ready.insert(i);
        }
    }
    let mut rank = vec![0; n];
    let mut count = 0;
    while let Some(i) = ready.pop_first() {
        rank[i] = count;
        count += 1;
        for &j in &upstream[i] {
            ready.insert(j);
        }
    }
    if count != n {
        return Err(PrepareError::Invalid("drainage cycle"));
    }
    Ok(rank)
}
impl HydrologyPlan {
    /// Parent stocks are inclusive. Count hierarchy roots only, never both a
    /// merged lake and its child reservoirs.
    pub fn seasonal_storage_m3(&self) -> [f64; 4] {
        let mut result = [0.0; 4];
        for d in self.depressions.iter().filter(|d| d.parent.is_none()) {
            for (s, v) in result.iter_mut().enumerate() {
                *v += d.storage_m3[s];
            }
        }
        result
    }
    pub fn seasonal_mean_q_m3_s(&self, i: usize) -> [f64; 4] {
        self.discharge_m3[i].map(|v| v / SECONDS_PER_SEASON)
    }
    pub fn annual_mean_q_m3_s(&self, i: usize) -> f64 {
        self.discharge_m3[i].iter().sum::<f64>() / SECONDS_PER_MODEL_YEAR
    }
    pub fn validate(
        &self,
        grid: &Grid,
        dem: &[f64],
        ocean: &[bool],
        local_m3: &[[f64; 4]],
    ) -> Result<(), PrepareError> {
        grid.validate()?;
        let n = grid.len();
        if dem.len() != n
            || ocean.len() != n
            || !dem.iter().all(|v| v.is_finite())
            || !arrays(grid, local_m3)
            || self.downstream.len() != n
            || self.rank.len() != n
            || self.water_level_m.len() != n
            || self.seasonal_water_level_m.len() != n
            || self.depression.len() != n
            || !arrays(grid, &self.discharge_m3)
            || !arrays(grid, &self.evaporation_m3)
            || !arrays(grid, &self.groundwater_export_m3)
            || !arrays(grid, &self.lake_evaporation_m3)
            || !arrays(grid, &self.lake_groundwater_export_m3)
            || self.rank != topo_rank(&self.downstream)?
        {
            return Err(PrepareError::Invalid(
                "invalid hydrology plan arrays or topology",
            ));
        }
        for i in 0..n {
            if let Some(j) = self.downstream[i] {
                if !grid.neighbours(i).any(|k| k == j)
                    || self.rank[j] >= self.rank[i]
                    || (dem[j] > dem[i] && self.depression[i].is_none())
                {
                    return Err(PrepareError::Invalid("invalid physical drainage edge"));
                }
            } else if !ocean[i] && !grid.boundary(i) {
                return Err(PrepareError::Invalid("unmarked interior outlet"));
            }
            if self.depression[i].is_some_and(|d| d >= self.depressions.len()) {
                return Err(PrepareError::Invalid("unknown depression"));
            }
            if ocean[i] && self.downstream[i].is_some() {
                return Err(PrepareError::Invalid("ocean seed is not an ocean outlet"));
            }
            for h in self.seasonal_water_level_m[i].into_iter().flatten() {
                if !h.is_finite() || h <= dem[i] {
                    return Err(PrepareError::Invalid("invalid lake surface"));
                }
            }
            if self.water_level_m[i] != self.seasonal_water_level_m[i][3] {
                return Err(PrepareError::Invalid("inconsistent final lake surface"));
            }
        }
        let mut area = vec![0.0; self.depressions.len()];
        let mut owner = vec![None; n];
        for (id, d) in self.depressions.iter().enumerate() {
            if id != d.id
                || !d.minimum_m.is_finite()
                || !d.spill_level_m.is_finite()
                || d.minimum_m > d.spill_level_m
                || !nonnegative(d.capacity_m3)
                || (d.cells.is_empty() && d.children.is_empty())
                || d.cells.iter().any(|i| *i >= n)
                || d.cells.windows(2).any(|v| v[0] >= v[1])
                || d.spill_from >= n
                || d.spill_to >= n
                || d.parent
                    .is_some_and(|p| p <= id || p >= self.depressions.len())
                || d.parent
                    .is_some_and(|p| !self.depressions[p].children.contains(&id))
                || (!d.children.is_empty()
                    && (d.children.len() != 2 || d.children[0] == d.children[1]))
                || d.children
                    .iter()
                    .any(|c| *c >= id || self.depressions[*c].parent != Some(id))
                || !d.storage_m3.into_iter().all(nonnegative)
                || !d.inflow_m3.into_iter().all(nonnegative)
                || !d.outflow_m3.into_iter().all(nonnegative)
                || !d.evaporation_m3.into_iter().all(nonnegative)
                || !d.groundwater_export_m3.into_iter().all(nonnegative)
                || d.level_m.into_iter().flatten().any(|h| !h.is_finite())
            {
                return Err(PrepareError::Invalid("invalid depression hierarchy"));
            }
            for &i in &d.cells {
                if owner[i].replace(id).is_some() || dem[i] > d.spill_level_m {
                    return Err(PrepareError::Invalid(
                        "duplicate or invalid depression cell owner",
                    ));
                }
            }
            if d.children
                .iter()
                .any(|c| self.depressions[*c].spill_level_m > d.spill_level_m)
            {
                return Err(PrepareError::Invalid("descending depression merge saddle"));
            }
            area[id] = d.cells.iter().map(|i| grid.area(*i)).sum::<f64>()
                + d.children.iter().map(|c| area[*c]).sum::<f64>();
            let capacity: f64 = d
                .cells
                .iter()
                .map(|i| (d.spill_level_m - dem[*i]) * grid.area(*i))
                .sum::<f64>()
                + d.children
                    .iter()
                    .map(|c| {
                        self.depressions[*c].capacity_m3
                            + (d.spill_level_m - self.depressions[*c].spill_level_m) * area[*c]
                    })
                    .sum::<f64>();
            if !close(capacity, d.capacity_m3) {
                return Err(PrepareError::Invalid("depression hypsometry capacity"));
            }
            for s in 0..4 {
                if d.storage_m3[s] > d.capacity_m3 + 1e-8 {
                    return Err(PrepareError::Invalid("lake capacity exceeded"));
                }
                let prior = if s == 0 { 0.0 } else { d.storage_m3[s - 1] };
                if !close(
                    prior + d.inflow_m3[s],
                    d.storage_m3[s]
                        + d.outflow_m3[s]
                        + d.evaporation_m3[s]
                        + d.groundwater_export_m3[s],
                ) {
                    return Err(PrepareError::Invalid("depression seasonal water balance"));
                }
            }
        }
        let ocean_outlets: Vec<_> = (0..n)
            .filter(|i| self.downstream[*i].is_none() && ocean[*i])
            .collect();
        let external_outlets: Vec<_> = (0..n)
            .filter(|i| self.downstream[*i].is_none() && !ocean[*i])
            .collect();
        if self.ocean_outlets != ocean_outlets || self.external_outlets != external_outlets {
            return Err(PrepareError::Invalid(
                "inconsistent physical outlet catalog",
            ));
        }
        let mut root_for_node = vec![0; self.depressions.len()];
        for id in (0..self.depressions.len()).rev() {
            root_for_node[id] = self.depressions[id].parent.map_or(id, |p| root_for_node[p]);
        }
        for i in 0..n {
            let expected_root = owner[i]
                .map(|id| root_for_node[id])
                .filter(|r| self.depressions[*r].capacity_m3 > 0.0);
            if self.depression[i] != expected_root {
                return Err(PrepareError::Invalid(
                    "inconsistent collapsed lake membership",
                ));
            }
            if expected_root.is_none()
                && (self.lake_evaporation_m3[i]
                    .iter()
                    .chain(&self.lake_groundwater_export_m3[i])
                    .any(|v| *v > 0.0))
            {
                return Err(PrepareError::Invalid("lake loss outside a reservoir"));
            }
            for (s, &local) in local_m3[i].iter().enumerate() {
                let incoming: f64 = grid
                    .neighbours(i)
                    .filter(|j| self.downstream[*j] == Some(i))
                    .map(|j| self.discharge_m3[j][s])
                    .sum();
                let storage_change = self.depression[i]
                    .filter(|r| self.depressions[*r].spill_from == i)
                    .map_or(0.0, |r| {
                        self.depressions[r].storage_m3[s]
                            - if s == 0 {
                                0.0
                            } else {
                                self.depressions[r].storage_m3[s - 1]
                            }
                    });
                let lake_loss = self.depression[i]
                    .filter(|r| self.depressions[*r].spill_from == i)
                    .map_or(0.0, |r| {
                        self.depressions[r].evaporation_m3[s]
                            + self.depressions[r].groundwater_export_m3[s]
                    });
                if !close(
                    local + incoming,
                    self.discharge_m3[i][s]
                        + self.evaporation_m3[i][s]
                        + self.groundwater_export_m3[i][s]
                        + storage_change
                        + lake_loss,
                ) {
                    return Err(PrepareError::Invalid("cell outgoing water balance"));
                }
            }
        }
        let roots: Vec<_> = self
            .depressions
            .iter()
            .filter(|d| d.parent.is_none())
            .collect();
        let mut prior = 0.0;
        for s in 0..4 {
            let input: f64 = local_m3.iter().map(|v| v[s]).sum();
            let stock: f64 = roots.iter().map(|d| d.storage_m3[s]).sum();
            let loss: f64 = self
                .evaporation_m3
                .iter()
                .chain(&self.groundwater_export_m3)
                .chain(&self.lake_evaporation_m3)
                .chain(&self.lake_groundwater_export_m3)
                .map(|v| v[s])
                .sum();
            let root_evaporation: f64 = roots.iter().map(|d| d.evaporation_m3[s]).sum();
            let root_groundwater: f64 = roots.iter().map(|d| d.groundwater_export_m3[s]).sum();
            if !close(
                root_evaporation,
                self.lake_evaporation_m3.iter().map(|v| v[s]).sum(),
            ) || !close(
                root_groundwater,
                self.lake_groundwater_export_m3.iter().map(|v| v[s]).sum(),
            ) {
                return Err(PrepareError::Invalid("lake loss ownership balance"));
            }
            let external: f64 = self
                .external_outlets
                .iter()
                .map(|i| self.discharge_m3[*i][s])
                .sum();
            let ocean_flow: f64 = self
                .ocean_outlets
                .iter()
                .map(|i| self.discharge_m3[*i][s])
                .sum();
            if !close(external, self.external_outflow_m3[s])
                || !close(ocean_flow, self.ocean_outflow_m3[s])
            {
                return Err(PrepareError::Invalid("terminal water balance"));
            }
            if !nonnegative(self.external_outflow_m3[s])
                || !nonnegative(self.ocean_outflow_m3[s])
                || !close(
                    input + prior,
                    stock + loss + self.external_outflow_m3[s] + self.ocean_outflow_m3[s],
                )
            {
                return Err(PrepareError::Invalid("domain seasonal water balance"));
            }
            prior = stock;
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeltaSplit {
    pub source_cell: usize,
    pub receiving_cells: Vec<usize>,
    pub fractions: Vec<f64>,
    pub discharge_m3: Vec<[f64; 4]>,
}
/// Explicit sediment-regime caller chooses branches. This conserves one source
/// flow, rather than copying it into every distributary; it does not infer delta geometry.
pub fn split_delta(
    plan: &HydrologyPlan,
    source_cell: usize,
    receiving_cells: &[usize],
    fractions: &[f64],
) -> Result<DeltaSplit, PrepareError> {
    if source_cell >= plan.discharge_m3.len()
        || receiving_cells.len() < 2
        || receiving_cells.len() != fractions.len()
        || receiving_cells
            .iter()
            .any(|c| *c >= plan.discharge_m3.len() || *c == source_cell)
        || receiving_cells
            .iter()
            .copied()
            .collect::<BTreeSet<_>>()
            .len()
            != receiving_cells.len()
        || !fractions.iter().copied().all(nonnegative)
        || !close(fractions.iter().sum(), 1.0)
    {
        return Err(PrepareError::Invalid("invalid delta split"));
    }
    let mut flow: Vec<[f64; 4]> = fractions
        .iter()
        .map(|f| plan.discharge_m3[source_cell].map(|v| v * f))
        .collect();
    for s in 0..4 {
        let total: f64 = flow[..flow.len() - 1].iter().map(|v| v[s]).sum();
        let last = flow.len() - 1;
        flow[last][s] = plan.discharge_m3[source_cell][s] - total;
        if !nonnegative(flow[last][s]) {
            return Err(PrepareError::Invalid("delta roundoff exceeds source"));
        }
    }
    Ok(DeltaSplit {
        source_cell,
        receiving_cells: receiving_cells.to_vec(),
        fractions: fractions.to_vec(),
        discharge_m3: flow,
    })
}

#[cfg(test)]
#[path = "hydrology_tests.rs"]
mod tests;
