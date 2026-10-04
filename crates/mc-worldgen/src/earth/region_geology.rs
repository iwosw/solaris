//! Physical geological interfaces and vegetation-stage inputs, never block states.
use serde::{Deserialize, Serialize};

use super::grid::Grid;
use super::prepared::PrepareError;
use super::tectonics::{Point, ProvinceKind, TectonicPlan};
use super::world_plan::PlanePlan;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RockClass {
    Basalt,
    Granite,
    Intrusive,
    Metamorphic,
    Carbonate,
    Sandstone,
    Shale,
    CoarseSediment,
    FineSediment,
    Soil,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyBand {
    pub bottom_m: f64,
    pub top_m: f64,
    pub lithology: RockClass,
    /// Synthetic millions of years before present, older downward.
    pub age_ma: f64,
    pub permeability_m_s: f64,
    pub solubility: f64,
    pub erodibility: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyProvenance {
    pub root_cell: usize,
    pub weight: f64,
    pub plate_id: usize,
    pub lithology_id: usize,
    pub province_id: Option<usize>,
    pub boundary_id: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyColumn {
    /// Basement, lower/upper sedimentary cover, unconsolidated deposits, soil.
    /// A zero-thickness band is absent, not an inverted or invented layer.
    pub bands: [GeologyBand; 5],
    pub provenance: [GeologyProvenance; 4],
    pub folding: f64,
    pub fold_axis_rad: f64,
    pub fault_displacement_m: f64,
    pub intrusion_fraction: f64,
    pub mineralization: f64,
    pub cold_phase_fraction: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EcologyInputs {
    pub temperature_c: [f64; 4],
    pub precipitation_mm: [f64; 4],
    pub potential_et_mm: [f64; 4],
    pub moisture_fraction: [f64; 4],
    pub snow_water_mm: [f64; 4],
    pub snow_fraction: [f64; 4],
    /// Fraction of the four seasons inundated: a proxy, not a new water budget.
    pub hydroperiod_fraction: f64,
    pub water_deficit_mm: [f64; 4],
    pub soil_depth_m: f64,
    pub rock_exposure: f64,
    pub slope: f64,
    pub wet: bool,
    pub ocean_fraction: f64,
    pub shore_fraction: f64,
    pub parent_material: RockClass,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyEcologySample {
    pub geology: GeologyColumn,
    pub ecology: EcologyInputs,
}

/// Prepared once with the root, so local sampling never searches a feature catalog.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyContext {
    pub cells: Vec<GeologyHistoryCell>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeologyHistoryCell {
    pub province_id: Option<usize>,
    pub boundary_id: Option<usize>,
    pub sediment_age_ma: f64,
    pub folding: f64,
    pub axis_rad: f64,
    pub origin: Point,
    pub fault_displacement_m: f64,
    pub intrusion_fraction: f64,
    pub mineralization: f64,
    pub basement: RockClass,
}

impl GeologyContext {
    pub fn build(grid: &Grid, tectonics: &TectonicPlan) -> Result<Self, PrepareError> {
        grid.validate()?;
        let mut cells = Vec::with_capacity(grid.len());
        Self::visit_cells(grid, tectonics, |_, cell| {
            cells.push(cell);
            Ok(())
        })?;
        let context = Self { cells };
        context.validate(grid)?;
        Ok(context)
    }

    /// Cold root validation regenerates one record at a time, not a second root-sized buffer.
    pub fn validate_provenance(
        &self,
        grid: &Grid,
        tectonics: &TectonicPlan,
    ) -> Result<(), PrepareError> {
        self.validate(grid)?;
        Self::visit_cells(grid, tectonics, |i, cell| {
            if self.cells[i] != cell {
                return Err(PrepareError::Invalid("root geology provenance mismatch"));
            }
            Ok(())
        })
    }

    fn visit_cells(
        grid: &Grid,
        tectonics: &TectonicPlan,
        mut visit: impl FnMut(usize, GeologyHistoryCell) -> Result<(), PrepareError>,
    ) -> Result<(), PrepareError> {
        grid.validate()?;
        if tectonics.crust.len() != grid.len() {
            return Err(PrepareError::Invalid("geology crust length mismatch"));
        }
        for i in 0..grid.len() {
            let (x_m, z_m) = grid.center(i);
            let crust = &tectonics.crust[i];
            let rock = tectonics
                .lithologies
                .iter()
                .find(|r| r.id == crust.lithology_id)
                .ok_or(PrepareError::Invalid("geology unknown basement lithology"))?;
            let basement = match rock.name.as_str() {
                "basalt" | "rift_volcanics" => RockClass::Basalt,
                "metamorphic_fold_belt" => RockClass::Metamorphic,
                "arc_intrusive" => RockClass::Intrusive,
                _ => {
                    if crust.continental_fraction < 0.5 {
                        RockClass::Basalt
                    } else {
                        RockClass::Granite
                    }
                }
            };
            let mut h = GeologyHistoryCell {
                province_id: None,
                boundary_id: None,
                sediment_age_ma: crust.basement_age_ma * 0.25,
                folding: 0.0,
                axis_rad: 0.0,
                origin: Point { x_m, z_m },
                fault_displacement_m: 0.0,
                intrusion_fraction: 0.0,
                mineralization: 0.0,
                basement,
            };
            let mut dominant = 0.0;
            for p in &tectonics.provinces {
                if p.kind == ProvinceKind::ColdPhase {
                    continue;
                }
                let (s, c) = p.footprint.angle_rad.sin_cos();
                let dx = x_m - p.footprint.center.x_m;
                let dz = z_m - p.footprint.center.z_m;
                let r2 = ((dx * c + dz * s) / p.footprint.radius_x_m).powi(2)
                    + ((-dx * s + dz * c) / p.footprint.radius_z_m).powi(2);
                if r2 > 9.0 {
                    continue;
                }
                let w = (-2.0 * r2).exp();
                if w > dominant {
                    dominant = w;
                    h.province_id = Some(p.id);
                    h.sediment_age_ma = p.age_ma.min(crust.basement_age_ma) * 0.5;
                    h.folding = p.folding * w;
                    h.axis_rad = p.footprint.angle_rad;
                    h.origin = p.footprint.center;
                    h.intrusion_fraction = p.intrusion_fraction * w;
                    h.mineralization = p.mineralization * w;
                }
            }
            let mut strongest_structure = 0.0;
            for b in &tectonics.boundaries {
                let structure = tectonics
                    .structures
                    .iter()
                    .find(|s| s.boundary_id == b.id)
                    .ok_or(PrepareError::Invalid("geology missing boundary structure"))?;
                for line in b.points.windows(2) {
                    let dx = line[1].x_m - line[0].x_m;
                    let dz = line[1].z_m - line[0].z_m;
                    let length2 = dx * dx + dz * dz;
                    if length2 == 0.0 {
                        continue;
                    }
                    let u = (((x_m - line[0].x_m) * dx + (z_m - line[0].z_m) * dz) / length2)
                        .clamp(0.0, 1.0);
                    let near = Point {
                        x_m: line[0].x_m + u * dx,
                        z_m: line[0].z_m + u * dz,
                    };
                    let distance = (x_m - near.x_m).hypot(z_m - near.z_m);
                    let w = (-(distance / 100_000.0).powi(2)).exp();
                    if w > strongest_structure {
                        strongest_structure = w;
                        h.boundary_id = Some(b.id);
                        // Retain inherited folding unless modern compression dominates.
                        if structure.folding * w > h.folding {
                            h.folding = structure.folding * w;
                            h.axis_rad = dz.atan2(dx);
                            h.origin = near;
                        }
                        let signed =
                            (-dz * (x_m - near.x_m) + dx * (z_m - near.z_m)) / length2.sqrt();
                        h.fault_displacement_m =
                            structure.fault_displacement_m.clamp(-2000.0, 2000.0)
                                * w
                                * (signed / grid.step_m.max(1.0)).tanh();
                        h.intrusion_fraction =
                            h.intrusion_fraction.max(structure.intrusion_fraction * w);
                        h.mineralization = h.mineralization.max(structure.mineralization * w);
                    }
                }
            }
            h.basement = if h.intrusion_fraction > 0.5 {
                RockClass::Intrusive
            } else if h.folding > 0.5 {
                RockClass::Metamorphic
            } else {
                h.basement
            };
            visit(i, h)?;
        }
        Ok(())
    }

    pub fn validate(&self, grid: &Grid) -> Result<(), PrepareError> {
        if self.cells.len() != grid.len()
            || self.cells.iter().any(|c| {
                ![
                    c.sediment_age_ma,
                    c.folding,
                    c.axis_rad,
                    c.origin.x_m,
                    c.origin.z_m,
                    c.fault_displacement_m,
                    c.intrusion_fraction,
                    c.mineralization,
                ]
                .into_iter()
                .all(f64::is_finite)
                    || c.sediment_age_ma < 0.0
                    || [c.folding, c.intrusion_fraction, c.mineralization]
                        .into_iter()
                        .any(|v| !(0.0..=1.0).contains(&v))
            })
        {
            return Err(PrepareError::Invalid("invalid root geology context"));
        }
        Ok(())
    }
}

/// Interpolates only four physical cell centres, including partial outer cells.
fn axis(p: f64, min: f64, extent: f64, step: f64, count: usize) -> (usize, usize, f64) {
    if count == 1 {
        return (0, 0, 0.0);
    }
    let a = ((p - min) / step - 0.5)
        .floor()
        .clamp(0.0, (count - 2) as f64) as usize;
    let center = |i: usize| min + i as f64 * step + step.min(extent - i as f64 * step) * 0.5;
    let lo = center(a);
    let hi = center(a + 1);
    let t = ((p - lo) / (hi - lo)).clamp(0.0, 1.0);
    (a, a + 1, t)
}

fn band(
    bottom_m: f64,
    top_m: f64,
    lithology: RockClass,
    age_ma: f64,
    inherited_erodibility: f64,
) -> GeologyBand {
    let (permeability_m_s, solubility, erodibility) = match lithology {
        RockClass::Basalt => (1e-8, 0.01, inherited_erodibility),
        RockClass::Granite | RockClass::Intrusive => (1e-9, 0.01, inherited_erodibility),
        RockClass::Metamorphic => (3e-9, 0.02, inherited_erodibility),
        RockClass::Carbonate => (1e-6, 0.95, 0.35),
        RockClass::Sandstone => (5e-6, 0.05, 0.45),
        RockClass::Shale => (1e-10, 0.03, 0.65),
        RockClass::CoarseSediment => (1e-4, 0.02, 0.9),
        RockClass::FineSediment => (1e-7, 0.03, 0.85),
        RockClass::Soil => (2e-6, 0.03, 0.95),
    };
    GeologyBand {
        bottom_m,
        top_m,
        lithology,
        age_ma,
        permeability_m_s,
        solubility,
        erodibility,
    }
}

pub fn sample(
    root: &PlanePlan,
    x_m: f64,
    z_m: f64,
    ground_m: f64,
    wet: bool,
) -> GeologyEcologySample {
    let g = &root.grid;
    let (x0, x1, tx) = axis(x_m, g.min_x_m, g.width_m, g.step_m, g.columns);
    let (z0, z1, tz) = axis(z_m, g.min_z_m, g.depth_m, g.step_m, g.rows);
    let ids = [
        z0 * g.columns + x0,
        z0 * g.columns + x1,
        z1 * g.columns + x0,
        z1 * g.columns + x1,
    ];
    let weights = [
        (1.0 - tx) * (1.0 - tz),
        tx * (1.0 - tz),
        (1.0 - tx) * tz,
        tx * tz,
    ];
    let blend =
        |f: &dyn Fn(usize) -> f64| ids.iter().zip(weights).map(|(&i, w)| w * f(i)).sum::<f64>();
    let history = &root.region_geology.cells;
    let provenance = std::array::from_fn(|k| {
        let i = ids[k];
        GeologyProvenance {
            root_cell: i,
            weight: weights[k],
            plate_id: root.tectonics.plate_id[i],
            lithology_id: root.tectonics.crust[i].lithology_id,
            province_id: history[i].province_id,
            boundary_id: history[i].boundary_id,
        }
    });
    let elevation = blend(&|i| root.elevation_m[i]);
    // Interpolate centre gradients rather than differentiating a bilinear patch:
    // the latter jumps at centre knots and would create discontinuous soil interfaces.
    // Each of four corners reads at most four immediate root neighbours.
    let gradients = ids.map(|i| {
        let x = i % g.columns;
        let z = i / g.columns;
        let west = z * g.columns + x.saturating_sub(1);
        let east = z * g.columns + (x + 1).min(g.columns - 1);
        let north = z.saturating_sub(1) * g.columns + x;
        let south = (z + 1).min(g.rows - 1) * g.columns + x;
        let dx = g.center(east).0 - g.center(west).0;
        let dz = g.center(south).1 - g.center(north).1;
        [
            if dx > 0.0 {
                (root.elevation_m[east] - root.elevation_m[west]) / dx
            } else {
                0.0
            },
            if dz > 0.0 {
                (root.elevation_m[south] - root.elevation_m[north]) / dz
            } else {
                0.0
            },
        ]
    });
    let gradient_x = gradients
        .iter()
        .zip(weights)
        .map(|(v, w)| v[0] * w)
        .sum::<f64>();
    let gradient_z = gradients
        .iter()
        .zip(weights)
        .map(|(v, w)| v[1] * w)
        .sum::<f64>();
    let slope = gradient_x.hypot(gradient_z);
    let folding = blend(&|i| history[i].folding);
    let fault_displacement_m = blend(&|i| history[i].fault_displacement_m);
    let fold_offset = blend(&|i| {
        let h = &history[i];
        let (s, c) = h.axis_rad.sin_cos();
        let across = -(x_m - h.origin.x_m) * s + (z_m - h.origin.z_m) * c;
        h.folding
            * root.tectonics.crust[i].sediment_cover_m.min(1500.0)
            * 0.25
            * (std::f64::consts::TAU * across / 40_000.0).sin()
    });
    let fold_axis_rad = blend(&|i| (2.0 * history[i].axis_rad).sin())
        .atan2(blend(&|i| (2.0 * history[i].axis_rad).cos()))
        * 0.5;
    let basement_age = blend(&|i| root.tectonics.crust[i].basement_age_ma);
    let sediment_age = blend(&|i| history[i].sediment_age_ma).min(basement_age);
    let sediment_cover = blend(&|i| root.tectonics.crust[i].sediment_cover_m);
    let tectonic_surface = blend(&|i| root.tectonics.elevation_m[i]);
    let geological_base = tectonic_surface - sediment_cover + fold_offset + fault_displacement_m;
    let geological_middle = geological_base + sediment_cover * 0.65;
    let mut deposits = [0.0; 2];
    // Root revision fixes two passes. Never walk upstream or an unbounded history.
    for pass in root.erosion_passes.iter().take(2) {
        let removal = blend(&|i| pass.source_m3[i].iter().sum::<f64>() / g.area(i));
        let retained =
            (deposits[0] + deposits[1] - removal).max(0.0) / (deposits[0] + deposits[1]).max(1e-12);
        for (class, stock) in deposits.iter_mut().enumerate() {
            *stock = *stock * retained + blend(&|i| pass.deposited_m3[i][class] / g.area(i));
        }
    }
    let ocean_fraction = blend(&|i| if root.ocean[i] { 1.0 } else { 0.0 });
    let temperature_c = std::array::from_fn(|s| {
        blend(&|i| root.climate_forcing.temperature_c[i][s]) - 0.006 * (ground_m - elevation)
    });
    let precipitation_mm =
        std::array::from_fn(|s| blend(&|i| root.climate_forcing.precipitation_mm[i][s]));
    let potential_et_mm =
        std::array::from_fn(|s| blend(&|i| root.climate_forcing.potential_et_mm[i][s]));
    let moisture_supply =
        precipitation_mm.iter().sum::<f64>() / (potential_et_mm.iter().sum::<f64>() + 1.0);
    let warmth = ((temperature_c.iter().sum::<f64>() / 4.0 + 10.0) / 30.0).clamp(0.0, 1.0);
    let inherited_erodibility = blend(&|i| root.tectonics.erodibility[i]);
    let stability = 1.0 / (1.0 + (slope / 0.35).powi(2));
    let soil_depth_m =
        ((0.1 + 1.4 * inherited_erodibility + (deposits[0] + deposits[1]).min(3.0) * 0.2)
            * warmth
            * moisture_supply.min(1.0)
            * stability
            * if wet { 0.25 } else { 1.0 })
        .min(2.5)
            * (1.0 - ocean_fraction);
    let soil_bottom = ground_m - soil_depth_m;
    let unconsolidated_bottom = soil_bottom - deposits.iter().sum::<f64>();
    let basement_top = geological_base.min(unconsolidated_bottom);
    let middle = geological_middle
        .min(unconsolidated_bottom)
        .max(basement_top);
    let dominant = (0..4)
        .max_by(|&a, &b| {
            weights[a]
                .total_cmp(&weights[b])
                .then_with(|| ids[b].cmp(&ids[a]))
        })
        .unwrap();
    let basement = history[ids[dominant]].basement;
    let lower_sediment = if blend(&|i| root.tectonics.crust[i].continental_fraction) < 0.5 {
        RockClass::Carbonate
    } else {
        RockClass::Sandstone
    };
    let deposit_rock = if deposits[0] > deposits[1] {
        RockClass::CoarseSediment
    } else {
        RockClass::FineSediment
    };
    let bands = [
        band(
            basement_top - blend(&|i| root.tectonics.crust[i].thickness_m),
            basement_top,
            basement,
            basement_age,
            inherited_erodibility,
        ),
        band(
            basement_top,
            middle,
            lower_sediment,
            sediment_age,
            inherited_erodibility,
        ),
        band(
            middle,
            unconsolidated_bottom,
            RockClass::Shale,
            sediment_age * 0.5,
            inherited_erodibility,
        ),
        band(
            unconsolidated_bottom,
            soil_bottom,
            deposit_rock,
            (sediment_age * 0.25).min(0.01),
            inherited_erodibility,
        ),
        band(
            soil_bottom,
            ground_m,
            RockClass::Soil,
            0.0,
            inherited_erodibility,
        ),
    ];
    let parent_band = bands[..4]
        .iter()
        .rev()
        .find(|b| b.top_m > b.bottom_m)
        .unwrap();
    let drainage = (parent_band.permeability_m_s / 1e-6).sqrt();
    let hydroperiod_fraction = (0..4)
        .map(|s| {
            blend(&|i| {
                if root.ocean[i] {
                    return 1.0;
                }
                root.hydrology.seasonal_water_level_m[i][s].map_or(0.0, |level| {
                    (0.5 + (level - ground_m) / 2.0).clamp(0.0, 1.0)
                })
            })
        })
        .sum::<f64>()
        / 4.0;
    let hydroperiod_fraction = hydroperiod_fraction.max(if wet { 0.25 } else { 0.0 });
    let snow_water_mm =
        std::array::from_fn(|s| blend(&|i| root.runoff.snow_storage_m3[i][s] / g.area(i) * 1000.0));
    let snow_fraction = std::array::from_fn(|s| ((2.0 - temperature_c[s]) / 4.0).clamp(0.0, 1.0));
    let moisture_fraction = std::array::from_fn(|s| {
        let stored = blend(&|i| root.runoff.soil_storage_m3[i][s] / g.area(i) * 1000.0);
        let supplied = precipitation_mm[s] * (1.0 - snow_fraction[s])
            + blend(&|i| {
                (root.runoff.snowmelt_m3[i][s] + root.runoff.baseflow_m3[i][s]) / g.area(i) * 1000.0
            });
        ((stored + supplied)
            / ((potential_et_mm[s] + soil_depth_m * 200.0 + 1.0) * (1.0 + drainage))
            + hydroperiod_fraction)
            .clamp(0.0, 1.0)
    });
    let water_deficit_mm = std::array::from_fn(|s| {
        (potential_et_mm[s]
            - precipitation_mm[s] * (1.0 - snow_fraction[s])
            - blend(&|i| {
                (root.runoff.snowmelt_m3[i][s] + root.runoff.baseflow_m3[i][s]) / g.area(i) * 1000.0
            }))
        .max(0.0)
    });
    let shore_fraction = (4.0 * ocean_fraction * (1.0 - ocean_fraction))
        .max(ocean_fraction * (1.0 - (ground_m.abs() / 10.0).min(1.0)));
    let rock_exposure = (1.0 - (soil_depth_m + deposits.iter().sum::<f64>()) / 0.5).clamp(0.0, 1.0);
    let parent_material = parent_band.lithology;
    GeologyEcologySample {
        geology: GeologyColumn {
            bands,
            provenance,
            folding,
            fold_axis_rad,
            fault_displacement_m,
            intrusion_fraction: blend(&|i| history[i].intrusion_fraction),
            mineralization: blend(&|i| history[i].mineralization),
            cold_phase_fraction: blend(&|i| {
                if root.tectonics.cold_phase[i] {
                    1.0
                } else {
                    0.0
                }
            }),
        },
        ecology: EcologyInputs {
            temperature_c,
            precipitation_mm,
            potential_et_mm,
            moisture_fraction,
            snow_water_mm,
            snow_fraction,
            hydroperiod_fraction,
            water_deficit_mm,
            soil_depth_m,
            rock_exposure,
            slope,
            wet,
            ocean_fraction,
            shore_fraction,
            parent_material,
        },
    }
}

#[cfg(test)]
#[path = "region_geology_tests.rs"]
mod tests;
