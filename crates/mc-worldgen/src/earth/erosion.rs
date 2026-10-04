//! One bounded physical geomorphic pass; callers reroute climate/runoff after it.
//! Sediment volumes are solid-equivalent m³, ice is water-equivalent m³.
//! The two sediment columns are coarse and fine, never unaccounted visual sand.
use serde::{Deserialize, Serialize};

use super::grid::Grid;
use super::prepared::PrepareError;

const SEASON_DAYS: f64 = 365.0 / 4.0;
const MAX_CUT_M: f64 = 12.0;
const MAX_DEPOSIT_M: f64 = 20.0;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErosionClimate {
    pub temperature_c: Vec<[f64; 4]>,
    pub precipitation_mm: Vec<[f64; 4]>,
    pub potential_et_mm: Vec<[f64; 4]>,
    pub snow_fraction: Vec<[f64; 4]>,
    pub wind_m_s: Vec<[[f64; 2]; 4]>,
    /// Actual connected ocean surface; distinct from elevation below sea level.
    pub ocean: Vec<bool>,
}

impl ErosionClimate {
    pub fn validate(&self, grid: &Grid) -> Result<(), PrepareError> {
        grid.validate()?;
        let n = grid.len();
        if [
            self.temperature_c.len(),
            self.precipitation_mm.len(),
            self.potential_et_mm.len(),
            self.snow_fraction.len(),
            self.wind_m_s.len(),
            self.ocean.len(),
        ]
        .into_iter()
        .any(|v| v != n)
        {
            return Err(PrepareError::Invalid("erosion climate length mismatch"));
        }
        for i in 0..n {
            for s in 0..4 {
                let t = self.temperature_c[i][s];
                let p = self.precipitation_mm[i][s];
                let et = self.potential_et_mm[i][s];
                let snow = self.snow_fraction[i][s];
                if !t.is_finite()
                    || !p.is_finite()
                    || p < 0.0
                    || !et.is_finite()
                    || et < 0.0
                    || !snow.is_finite()
                    || !(0.0..=1.0).contains(&snow)
                    || !self.wind_m_s[i][s].into_iter().all(f64::is_finite)
                {
                    return Err(PrepareError::Invalid("invalid erosion climate value"));
                }
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum SedimentClass {
    Coarse,
    Fine,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub enum ValleyClass {
    Unaltered,
    Fluvial,
    InheritedGlacial,
    PresentGlacial,
}

/// A complete one-pass ice ledger, including retained (not transported) ice.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct IceBudget {
    pub source_m3: Vec<f64>,
    pub ablation_m3: Vec<f64>,
    pub transported_m3: Vec<f64>,
    pub exported_m3: Vec<f64>,
    pub stored_m3: Vec<f64>,
    pub residual_m3: f64,
}

impl IceBudget {
    fn empty(n: usize) -> Self {
        Self {
            source_m3: vec![0.0; n],
            ablation_m3: vec![0.0; n],
            transported_m3: vec![0.0; n],
            exported_m3: vec![0.0; n],
            stored_m3: vec![0.0; n],
            residual_m3: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ErosionPlan {
    pub elevation_m: Vec<f64>,
    pub sediment_classes: [SedimentClass; 2],
    pub erodibility: Vec<f64>,
    /// Inherited geological provenance, not a current snow/glacier mask.
    pub cold_phase: Vec<bool>,
    pub ocean: Vec<bool>,
    pub downstream: Vec<Option<usize>>,
    /// Per-cell rank; downstream has strictly smaller rank.
    pub rank: Vec<usize>,
    pub base_level_m: Vec<f64>,
    pub fluvial_erosion_m: Vec<f64>,
    pub glacial_erosion_m: Vec<f64>,
    pub hillslope_erosion_m: Vec<f64>,
    pub valley_class: Vec<ValleyClass>,
    pub source_m3: Vec<[f64; 2]>,
    pub deposited_m3: Vec<[f64; 2]>,
    /// Outbound sediment along downstream edges (not catchment totals).
    pub transported_m3: Vec<[f64; 2]>,
    pub exported_m3: Vec<[f64; 2]>,
    pub stored_m3: Vec<[f64; 2]>,
    pub hillslope_receiver: Vec<Option<usize>>,
    pub hillslope_transfer_m3: Vec<[f64; 2]>,
    pub eolian_receiver: Vec<Option<usize>>,
    pub eolian_transfer_m3: Vec<[f64; 2]>,
    /// Distributed transport opportunity from actual outgoing ice flux: 60%
    /// thalweg, at most 20% on each transverse bank. Not another ice source.
    pub ice_footprint_m3: Vec<f64>,
    pub present_ice: IceBudget,
    /// Reduced historical episode: positive local runoff feeds inherited cold
    /// accumulation; warm reaches ablate it. This is not reconstructed paleo-climate.
    pub inherited_ice: IceBudget,
    pub present_glacier: Vec<bool>,
    pub sediment_residual_m3: [f64; 2],
}

/// Fixed one-pass stream-power, ice abrasion, talus and dry-sediment wind transport.
/// Climate arrays must already be conservatively remapped to this physical grid.
pub fn erode(
    grid: &Grid,
    dem: &[f64],
    erodibility: &[f64],
    cold_phase: &[bool],
    routing: (&[Option<usize>], &[usize]),
    discharge_m3: &[[f64; 4]],
    climate: &ErosionClimate,
) -> Result<ErosionPlan, PrepareError> {
    let (downstream, rank) = routing;
    grid.validate()?;
    climate.validate(grid)?;
    let n = grid.len();
    if [
        dem.len(),
        erodibility.len(),
        cold_phase.len(),
        downstream.len(),
        rank.len(),
        discharge_m3.len(),
    ]
    .into_iter()
    .any(|v| v != n)
    {
        return Err(PrepareError::Invalid("erosion input length mismatch"));
    }
    let order = graph_order(downstream, rank)?;
    for i in 0..n {
        if !dem[i].is_finite()
            || !erodibility[i].is_finite()
            || !(0.0..=1.0).contains(&erodibility[i])
            || !discharge_m3[i]
                .into_iter()
                .all(|q| q.is_finite() && q >= 0.0)
        {
            return Err(PrepareError::Invalid("invalid erosion input value"));
        }
    }
    let mut plan = ErosionPlan {
        elevation_m: dem.to_vec(),
        sediment_classes: [SedimentClass::Coarse, SedimentClass::Fine],
        erodibility: erodibility.to_vec(),
        cold_phase: cold_phase.to_vec(),
        ocean: climate.ocean.clone(),
        downstream: downstream.to_vec(),
        rank: rank.to_vec(),
        base_level_m: (0..n)
            .map(|i| downstream[i].map_or(dem[i], |d| dem[i].min(dem[d])))
            .collect(),
        fluvial_erosion_m: vec![0.0; n],
        glacial_erosion_m: vec![0.0; n],
        hillslope_erosion_m: vec![0.0; n],
        valley_class: vec![ValleyClass::Unaltered; n],
        source_m3: vec![[0.0; 2]; n],
        deposited_m3: vec![[0.0; 2]; n],
        transported_m3: vec![[0.0; 2]; n],
        exported_m3: vec![[0.0; 2]; n],
        stored_m3: vec![[0.0; 2]; n],
        hillslope_receiver: vec![None; n],
        hillslope_transfer_m3: vec![[0.0; 2]; n],
        eolian_receiver: vec![None; n],
        eolian_transfer_m3: vec![[0.0; 2]; n],
        ice_footprint_m3: vec![0.0; n],
        present_ice: IceBudget::empty(n),
        inherited_ice: IceBudget::empty(n),
        present_glacier: vec![false; n],
        sediment_residual_m3: [0.0; 2],
    };
    let mut upstream_q = vec![0.0; n];
    for i in 0..n {
        if let Some(d) = downstream[i] {
            upstream_q[d] += discharge_m3[i].iter().sum::<f64>();
        }
    }
    for i in 0..n {
        let area = grid.area(i);
        if !climate.ocean[i] {
            plan.present_ice.source_m3[i] = (0..4)
                .map(|s| {
                    climate.precipitation_mm[i][s] * climate.snow_fraction[i][s] * 0.001 * area
                })
                .sum();
        }
        if cold_phase[i] {
            plan.inherited_ice.source_m3[i] =
                (discharge_m3[i].iter().sum::<f64>() - upstream_q[i]).max(0.0) * 0.25;
        }
    }
    route_ice(
        (grid, dem),
        downstream,
        &order,
        climate,
        cold_phase,
        false,
        &mut plan.present_ice,
    );
    route_ice(
        (grid, dem),
        downstream,
        &order,
        climate,
        cold_phase,
        true,
        &mut plan.inherited_ice,
    );
    // All excavation reads the original DEM. No scheduler-dependent in-place slope.
    for i in 0..n {
        let slope = slope(grid, dem, downstream, i);
        let q_depth = discharge_m3[i].iter().sum::<f64>() / grid.area(i);
        let cut = bounded_cut(
            &plan,
            dem,
            i,
            4.0 * erodibility[i] * q_depth.sqrt() * slope.sqrt(),
        );
        plan.fluvial_erosion_m[i] = cut;
        if cut > 0.0 {
            plan.valley_class[i] = ValleyClass::Fluvial;
        }
        let current_flux = if climate.ocean[i] {
            0.0
        } else {
            plan.present_ice.transported_m3[i] + plan.present_ice.exported_m3[i]
        };
        let inherited_flux =
            plan.inherited_ice.transported_m3[i] + plan.inherited_ice.exported_m3[i];
        plan.present_glacier[i] =
            current_flux > 0.0 && plan.present_ice.stored_m3[i] / grid.area(i) >= 0.1;
        let ice_depth = (current_flux + inherited_flux) / grid.area(i);
        if ice_depth > 0.0 {
            let cut = bounded_cut(
                &plan,
                dem,
                i,
                1.5 * erodibility[i] * ice_depth.sqrt() * slope.sqrt(),
            );
            plan.glacial_erosion_m[i] += cut;
            if cut > 0.0 {
                plan.valley_class[i] = if current_flux > inherited_flux {
                    ValleyClass::PresentGlacial
                } else {
                    ValleyClass::InheritedGlacial
                };
            }
        }
    }
    // Abrasion by a finite-width ice stream removes valley banks as well as its
    // thalweg; ordinary fluvial incision remains narrow, producing a V profile.
    for i in 0..n {
        let Some(d) = downstream[i] else { continue };
        let flux = plan.present_ice.transported_m3[i] + plan.inherited_ice.transported_m3[i];
        if flux <= 0.0 {
            continue;
        }
        plan.ice_footprint_m3[i] += 0.6 * flux;
        let (x, z) = grid.center(i);
        let (dx, dz) = grid.center(d);
        let along_x = (dx - x).abs() > (dz - z).abs();
        let floor = dem[i] - plan.fluvial_erosion_m[i] - plan.glacial_erosion_m[i];
        for bank in grid.neighbours(i) {
            let (bx, bz) = grid.center(bank);
            if (along_x && (bx - x).abs() > (bz - z).abs())
                || (!along_x && (bz - z).abs() > (bx - x).abs())
            {
                continue;
            }
            let bank_flux = if climate.ocean[bank] {
                plan.inherited_ice.transported_m3[i]
            } else {
                flux
            };
            plan.ice_footprint_m3[bank] += 0.2 * bank_flux;
            let strength = (0.35 * (bank_flux / grid.area(i)).sqrt() * erodibility[bank]).min(1.0);
            let proposed = (dem[bank] - floor).max(0.0) * strength;
            let cut = bounded_cut(&plan, dem, bank, proposed);
            plan.glacial_erosion_m[bank] += cut;
            if cut > 0.0 {
                plan.valley_class[bank] = if !climate.ocean[bank]
                    && plan.present_ice.transported_m3[i] > plan.inherited_ice.transported_m3[i]
                {
                    ValleyClass::PresentGlacial
                } else {
                    ValleyClass::InheritedGlacial
                };
            }
        }
    }
    // Material-dependent talus threshold: stable hard rock tolerates steeper
    // slopes than soft/weathered material. Fixed donor decisions, direct transfer.
    for i in 0..n {
        let mut receiver = None;
        let mut excess = 0.0;
        let angle_slope = 1.2 - 0.85 * erodibility[i];
        for j in grid.neighbours(i) {
            let threshold = angle_slope * distance(grid, i, j);
            let candidate = dem[i] - dem[j] - threshold;
            if candidate > excess {
                excess = candidate;
                receiver = Some(j);
            }
        }
        if let Some(j) = receiver {
            let cut = bounded_cut(&plan, dem, i, (excess * 0.1 * erodibility[i]).min(2.0));
            if cut > 0.0 {
                plan.hillslope_erosion_m[i] = cut;
                plan.hillslope_receiver[i] = Some(j);
                plan.hillslope_transfer_m3[i] = split_material(cut * grid.area(i), erodibility[i]);
            }
        }
        let cut =
            plan.fluvial_erosion_m[i] + plan.glacial_erosion_m[i] + plan.hillslope_erosion_m[i];
        plan.source_m3[i] = split_material(cut * grid.area(i), erodibility[i]);
    }
    for i in 0..n {
        if let Some(j) = plan.hillslope_receiver[i] {
            for c in 0..2 {
                plan.deposited_m3[j][c] += plan.hillslope_transfer_m3[i][c];
            }
        }
    }
    let mut incoming = vec![[0.0; 2]; n];
    for &i in order.iter().rev() {
        let q_depth = discharge_m3[i].iter().sum::<f64>() / grid.area(i);
        let slope = if downstream[i].is_none() && grid.boundary(i) {
            0.01
        } else {
            slope(grid, dem, downstream, i)
        };
        // Fine material remains suspended at lower stream power than coarse.
        let capacity = (q_depth.sqrt() + 4.0 * (plan.ice_footprint_m3[i] / grid.area(i)).sqrt())
            * slope.sqrt()
            * grid.area(i);
        let mut deposit_room =
            (MAX_DEPOSIT_M * grid.area(i) - plan.deposited_m3[i].iter().sum::<f64>()).max(0.0);
        for (c, inflow) in incoming[i].into_iter().enumerate() {
            let available =
                inflow + (plan.source_m3[i][c] - plan.hillslope_transfer_m3[i][c]).max(0.0);
            let carry = available.min(capacity * if c == 0 { 0.15 } else { 0.65 });
            let deposit = (available - carry).min(deposit_room);
            plan.deposited_m3[i][c] += deposit;
            deposit_room -= deposit;
            if let Some(d) = downstream[i] {
                plan.transported_m3[i][c] = carry;
                incoming[d][c] += carry;
                plan.stored_m3[i][c] = available - carry - deposit;
            } else if grid.boundary(i) {
                plan.exported_m3[i][c] = carry;
                plan.stored_m3[i][c] = available - carry - deposit;
            } else {
                // An interior closed sink cannot silently export solids.
                let extra_deposit = carry.min(deposit_room);
                plan.deposited_m3[i][c] += extra_deposit;
                deposit_room -= extra_deposit;
                plan.stored_m3[i][c] = available - deposit - extra_deposit;
            }
        }
    }
    // No bare-desert dunes: remobilize only actual loose fine deposition. P/PET
    // supplies an explicit climatic sparse-cover proxy, not runtime vegetation.
    for i in 0..n {
        if climate.ocean[i] {
            continue;
        }
        let p = climate.precipitation_mm[i].iter().sum::<f64>();
        let et = climate.potential_et_mm[i].iter().sum::<f64>();
        let dry = if et > 0.0 {
            (1.0 - 2.0 * p / et).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let mut wind = [0.0; 2];
        for s in 0..4 {
            for (axis, component) in wind.iter_mut().enumerate() {
                *component += climate.wind_m_s[i][s][axis] * 0.25;
            }
        }
        let speed = wind[0].hypot(wind[1]);
        if dry <= 0.0 || speed <= 3.0 {
            continue;
        }
        let (x, z) = grid.center(i);
        let mut best = 0.0;
        let mut receiver = None;
        for j in grid.neighbours(i) {
            let (jx, jz) = grid.center(j);
            // Climate vectors are east/north; physical grid +Z points south.
            let projection = (wind[0] * (jx - x) - wind[1] * (jz - z)) / distance(grid, i, j);
            if projection > best {
                best = projection;
                receiver = Some(j);
            }
        }
        if let Some(j) = receiver {
            let amount = plan.deposited_m3[i][1] * (dry * (speed - 3.0) * 0.025).min(0.5);
            if amount > 0.0 {
                plan.eolian_receiver[i] = Some(j);
                plan.eolian_transfer_m3[i][1] = amount;
            }
        }
    }
    // Separate write phase prevents wind from transporting a fresh neighbor's
    // deposit twice merely because its index happens to be later.
    for i in 0..n {
        if let Some(j) = plan.eolian_receiver[i] {
            plan.source_m3[i][1] += plan.eolian_transfer_m3[i][1];
            plan.deposited_m3[j][1] += plan.eolian_transfer_m3[i][1];
        }
    }
    for (i, &height) in dem.iter().enumerate() {
        plan.elevation_m[i] = height
            + (plan.deposited_m3[i].iter().sum::<f64>() - plan.source_m3[i].iter().sum::<f64>())
                / grid.area(i);
    }
    for c in 0..2 {
        plan.sediment_residual_m3[c] = plan.source_m3.iter().map(|v| v[c]).sum::<f64>()
            - (plan.deposited_m3.iter().map(|v| v[c]).sum::<f64>()
                + plan.exported_m3.iter().map(|v| v[c]).sum::<f64>()
                + plan.stored_m3.iter().map(|v| v[c]).sum::<f64>());
    }
    plan.validate(grid, dem)?;
    Ok(plan)
}

fn route_ice(
    surface: (&Grid, &[f64]),
    downstream: &[Option<usize>],
    order: &[usize],
    climate: &ErosionClimate,
    cold_phase: &[bool],
    inherited: bool,
    budget: &mut IceBudget,
) {
    let (grid, dem) = surface;
    let mut incoming = vec![0.0; grid.len()];
    for &i in order.iter().rev() {
        let available = incoming[i] + budget.source_m3[i];
        if !inherited && climate.ocean[i] {
            // Grounded ice reaching an actual ocean calves into its external
            // reservoir, even when that marine cell is interior in the grid.
            budget.exported_m3[i] = available;
            continue;
        }
        let melt = if inherited {
            available * if cold_phase[i] { 0.05 } else { 0.8 }
        } else {
            // Degree-day melt: 3 mm water equivalent per positive degree-day.
            climate.temperature_c[i]
                .iter()
                .map(|t| t.max(0.0) * SEASON_DAYS * 0.003 * grid.area(i))
                .sum::<f64>()
                .min(available)
        };
        budget.ablation_m3[i] = melt;
        let remaining = available - melt;
        let bed_slope = if downstream[i].is_none() && grid.boundary(i) {
            0.01
        } else {
            slope(grid, dem, downstream, i)
        };
        let mobility = (2.0 * bed_slope.sqrt()).min(0.9);
        if let Some(d) = downstream[i] {
            budget.transported_m3[i] = remaining * mobility;
            incoming[d] += budget.transported_m3[i];
        } else if grid.boundary(i) {
            budget.exported_m3[i] = remaining * mobility;
        }
        budget.stored_m3[i] = remaining - budget.transported_m3[i] - budget.exported_m3[i];
    }
    budget.residual_m3 = budget.source_m3.iter().sum::<f64>()
        - (budget.ablation_m3.iter().sum::<f64>()
            + budget.exported_m3.iter().sum::<f64>()
            + budget.stored_m3.iter().sum::<f64>());
}

fn graph_order(downstream: &[Option<usize>], rank: &[usize]) -> Result<Vec<usize>, PrepareError> {
    if downstream.len() != rank.len() {
        return Err(PrepareError::Invalid("erosion graph length mismatch"));
    }
    let n = rank.len();
    let mut order = vec![usize::MAX; n];
    for i in 0..n {
        if rank[i] >= n || order[rank[i]] != usize::MAX {
            return Err(PrepareError::Invalid("invalid erosion topological rank"));
        }
        order[rank[i]] = i;
        if let Some(d) = downstream[i]
            && (d >= n || rank[d] >= rank[i])
        {
            return Err(PrepareError::Invalid("invalid erosion downstream graph"));
        }
    }
    Ok(order)
}

fn distance(grid: &Grid, i: usize, j: usize) -> f64 {
    let (x, z) = grid.center(i);
    let (jx, jz) = grid.center(j);
    (jx - x).hypot(jz - z)
}
fn slope(grid: &Grid, dem: &[f64], downstream: &[Option<usize>], i: usize) -> f64 {
    downstream[i].map_or(0.0, |d| (dem[i] - dem[d]).max(0.0) / distance(grid, i, d))
}
fn bounded_cut(plan: &ErosionPlan, dem: &[f64], i: usize, proposed: f64) -> f64 {
    let already =
        plan.fluvial_erosion_m[i] + plan.glacial_erosion_m[i] + plan.hillslope_erosion_m[i];
    proposed
        .max(0.0)
        .min((MAX_CUT_M - already).max(0.0))
        .min((dem[i] - plan.base_level_m[i] - already).max(0.0))
}
fn split_material(volume: f64, erodibility: f64) -> [f64; 2] {
    let coarse = volume * (0.85 - 0.4 * erodibility);
    [coarse, volume - coarse]
}
fn close(a: f64, b: f64) -> bool {
    a.is_finite() && b.is_finite() && (a - b).abs() <= 1e-8 * a.abs().max(b.abs()).max(1.0)
}

impl ErosionPlan {
    pub fn validate(&self, grid: &Grid, original_dem: &[f64]) -> Result<(), PrepareError> {
        grid.validate()?;
        let n = grid.len();
        if [
            original_dem.len(),
            self.elevation_m.len(),
            self.erodibility.len(),
            self.cold_phase.len(),
            self.ocean.len(),
            self.downstream.len(),
            self.rank.len(),
            self.base_level_m.len(),
            self.fluvial_erosion_m.len(),
            self.glacial_erosion_m.len(),
            self.hillslope_erosion_m.len(),
            self.valley_class.len(),
            self.source_m3.len(),
            self.deposited_m3.len(),
            self.transported_m3.len(),
            self.exported_m3.len(),
            self.stored_m3.len(),
            self.hillslope_receiver.len(),
            self.hillslope_transfer_m3.len(),
            self.eolian_receiver.len(),
            self.eolian_transfer_m3.len(),
            self.ice_footprint_m3.len(),
            self.present_glacier.len(),
        ]
        .into_iter()
        .any(|v| v != n)
        {
            return Err(PrepareError::Invalid("erosion plan length mismatch"));
        }
        graph_order(&self.downstream, &self.rank)?;
        if self.sediment_classes != [SedimentClass::Coarse, SedimentClass::Fine] {
            return Err(PrepareError::Invalid("invalid sediment class catalog"));
        }
        let mut incoming = vec![[0.0; 2]; n];
        for i in 0..n {
            for (receiver, amounts) in [
                (self.downstream[i], self.transported_m3[i]),
                (self.hillslope_receiver[i], self.hillslope_transfer_m3[i]),
                (self.eolian_receiver[i], self.eolian_transfer_m3[i]),
            ] {
                if let Some(j) = receiver {
                    if j >= n || j == i {
                        return Err(PrepareError::Invalid("unknown erosion transfer cell"));
                    }
                    for c in 0..2 {
                        incoming[j][c] += amounts[c];
                    }
                } else if amounts != [0.0; 2] {
                    return Err(PrepareError::Invalid("sediment transfer without receiver"));
                }
            }
            for receiver in [self.hillslope_receiver[i], self.eolian_receiver[i]]
                .into_iter()
                .flatten()
            {
                if !grid.neighbours(i).any(|j| j == receiver) {
                    return Err(PrepareError::Invalid("nonlocal erosion transfer"));
                }
            }
        }
        for i in 0..n {
            let original = original_dem[i];
            let base = self.downstream[i].map_or(original, |d| original.min(original_dem[d]));
            let cut =
                self.fluvial_erosion_m[i] + self.glacial_erosion_m[i] + self.hillslope_erosion_m[i];
            if !original.is_finite()
                || !self.elevation_m[i].is_finite()
                || !self.erodibility[i].is_finite()
                || !(0.0..=1.0).contains(&self.erodibility[i])
                || !close(base, self.base_level_m[i])
                || !self.ice_footprint_m3[i].is_finite()
                || self.ice_footprint_m3[i] < 0.0
                || self.elevation_m[i] + 1e-8 < base
                || cut > MAX_CUT_M + 1e-8
                || cut > original - base + 1e-8
                || ![
                    self.fluvial_erosion_m[i],
                    self.glacial_erosion_m[i],
                    self.hillslope_erosion_m[i],
                ]
                .into_iter()
                .all(|v| v.is_finite() && v >= 0.0)
            {
                return Err(PrepareError::Invalid(
                    "invalid erosion elevation or base level",
                ));
            }
            let source = split_material(cut * grid.area(i), self.erodibility[i]);
            for c in 0..2 {
                if ![
                    self.source_m3[i][c],
                    self.deposited_m3[i][c],
                    self.transported_m3[i][c],
                    self.exported_m3[i][c],
                    self.stored_m3[i][c],
                    self.hillslope_transfer_m3[i][c],
                    self.eolian_transfer_m3[i][c],
                ]
                .into_iter()
                .all(|v| v.is_finite() && v >= 0.0)
                {
                    return Err(PrepareError::Invalid("invalid sediment volume"));
                }
                if !close(
                    self.source_m3[i][c],
                    source[c] + self.eolian_transfer_m3[i][c],
                ) || !close(
                    self.source_m3[i][c] + incoming[i][c],
                    self.deposited_m3[i][c]
                        + self.transported_m3[i][c]
                        + self.exported_m3[i][c]
                        + self.stored_m3[i][c]
                        + self.hillslope_transfer_m3[i][c]
                        + self.eolian_transfer_m3[i][c],
                ) {
                    return Err(PrepareError::Invalid("sediment cell balance mismatch"));
                }
                if self.exported_m3[i][c] > 0.0
                    && (!grid.boundary(i) || self.downstream[i].is_some())
                {
                    return Err(PrepareError::Invalid("sediment export at interior cell"));
                }
            }
            if self.eolian_transfer_m3[i][0] != 0.0
                || !close(
                    self.hillslope_transfer_m3[i].iter().sum(),
                    self.hillslope_erosion_m[i] * grid.area(i),
                )
                || !close(
                    self.elevation_m[i],
                    original
                        + (self.deposited_m3[i].iter().sum::<f64>()
                            - self.source_m3[i].iter().sum::<f64>())
                            / grid.area(i),
                )
            {
                return Err(PrepareError::Invalid("erosion geometry material mismatch"));
            }
        }
        for c in 0..2 {
            let total = self.source_m3.iter().map(|v| v[c]).sum::<f64>();
            let destination = self.deposited_m3.iter().map(|v| v[c]).sum::<f64>()
                + self.exported_m3.iter().map(|v| v[c]).sum::<f64>()
                + self.stored_m3.iter().map(|v| v[c]).sum::<f64>();
            if !close(total, destination)
                || !close(self.sediment_residual_m3[c], total - destination)
            {
                return Err(PrepareError::Invalid("global sediment balance mismatch"));
            }
        }
        validate_ice(&self.present_ice, grid, &self.downstream, Some(&self.ocean))?;
        validate_ice(&self.inherited_ice, grid, &self.downstream, None)?;
        for i in 0..n {
            if self.ocean[i]
                && (self.present_ice.source_m3[i] != 0.0
                    || self.present_ice.ablation_m3[i] != 0.0
                    || self.present_ice.transported_m3[i] != 0.0
                    || self.present_ice.stored_m3[i] != 0.0
                    || self.eolian_transfer_m3[i] != [0.0; 2]
                    || self.eolian_receiver[i].is_some())
            {
                return Err(PrepareError::Invalid(
                    "grounded ice or dry wind source on ocean",
                ));
            }
            if !self.cold_phase[i] && self.inherited_ice.source_m3[i] != 0.0 {
                return Err(PrepareError::Invalid(
                    "inherited ice without cold provenance",
                ));
            }
            let glacier = !self.ocean[i]
                && self.present_ice.transported_m3[i] + self.present_ice.exported_m3[i] > 0.0
                && self.present_ice.stored_m3[i] / grid.area(i) >= 0.1;
            if self.present_glacier[i] != glacier {
                return Err(PrepareError::Invalid("present glacier mass-flow mismatch"));
            }
        }
        Ok(())
    }
}

fn validate_ice(
    budget: &IceBudget,
    grid: &Grid,
    downstream: &[Option<usize>],
    marine_exports: Option<&[bool]>,
) -> Result<(), PrepareError> {
    let n = grid.len();
    if [
        budget.source_m3.len(),
        budget.ablation_m3.len(),
        budget.transported_m3.len(),
        budget.exported_m3.len(),
        budget.stored_m3.len(),
    ]
    .into_iter()
    .any(|v| v != n)
    {
        return Err(PrepareError::Invalid("ice ledger length mismatch"));
    }
    let mut incoming = vec![0.0; n];
    for (i, &target) in downstream.iter().enumerate() {
        if let Some(d) = target {
            incoming[d] += budget.transported_m3[i];
        } else if budget.transported_m3[i] != 0.0 {
            return Err(PrepareError::Invalid("ice flux without downstream"));
        }
    }
    for i in 0..n {
        if ![
            budget.source_m3[i],
            budget.ablation_m3[i],
            budget.transported_m3[i],
            budget.exported_m3[i],
            budget.stored_m3[i],
        ]
        .into_iter()
        .all(|v| v.is_finite() && v >= 0.0)
            || !close(
                budget.source_m3[i] + incoming[i],
                budget.ablation_m3[i]
                    + budget.transported_m3[i]
                    + budget.exported_m3[i]
                    + budget.stored_m3[i],
            )
        {
            return Err(PrepareError::Invalid("ice cell balance mismatch"));
        }
        if budget.exported_m3[i] > 0.0
            && !marine_exports.is_some_and(|ocean| ocean[i])
            && (!grid.boundary(i) || downstream[i].is_some())
        {
            return Err(PrepareError::Invalid("ice export at interior cell"));
        }
    }
    let total = budget.source_m3.iter().sum::<f64>();
    let destination = budget.ablation_m3.iter().sum::<f64>()
        + budget.exported_m3.iter().sum::<f64>()
        + budget.stored_m3.iter().sum::<f64>();
    if !close(total, destination) || !close(budget.residual_m3, total - destination) {
        return Err(PrepareError::Invalid("global ice balance mismatch"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "erosion_tests.rs"]
mod tests;
