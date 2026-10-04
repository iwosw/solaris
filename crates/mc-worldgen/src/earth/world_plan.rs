//! Coupled, fixed-pass physical root builder; no runtime blocks or derived caches.
use serde::{Deserialize, Serialize};

use super::basin_fixture::BasinFixture;
use super::climate::{self, ClimatePlan};
use super::definition::{WorldDefinition, hash};
use super::erosion::{self, ErosionClimate, ErosionPlan};
use super::grid::Grid;
use super::hydrology::{self, HydrologyPlan, RunoffPlan};
use super::prepared::PrepareError;
use super::region_geology::GeologyContext;
use super::region_water;
use super::tectonics::{self, TectonicPlan};

const MAGIC: &[u8; 8] = b"SOLROOT2";
/// Fixed in the pinned artifact, never selected by time, budgets or workers.
const EROSION_PASSES: usize = 2;

#[derive(Debug, Clone, PartialEq)]
pub enum WorldPlan {
    BasinFixture(Box<BasinFixture>),
    Plane(Box<PlanePlan>),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlanePlan {
    pub grid: Grid,
    pub climate_grid: Grid,
    pub tectonics: TectonicPlan,
    pub region_geology: GeologyContext,
    pub elevation_m: Vec<f64>,
    pub ocean: Vec<bool>,
    pub ocean_seeds: Vec<usize>,
    pub climate: ClimatePlan,
    pub climate_forcing: ErosionClimate,
    pub runoff: RunoffPlan,
    pub hydrology: HydrologyPlan,
    pub water_body_owner: Vec<[Option<usize>; 4]>,
    /// Canonical terminal/depression ownership, precomputed for local divide protection.
    pub basin_owner: Vec<usize>,
    pub erosion_passes: Vec<ErosionPlan>,
}

impl WorldPlan {
    pub fn build(definition: &WorldDefinition) -> Result<Self, PrepareError> {
        definition.check_preparation_support()?;
        if definition.input().refinement_profile == "w0-basin" {
            return Ok(Self::BasinFixture(Box::default()));
        }
        let d = definition.input();
        let grid = Grid::from_definition(definition, d.macro_cell_m)?;
        let climate_grid = Grid::from_definition(definition, d.climate_cell_m)?;
        let tectonics = tectonics::build(&grid, d.seed)?;
        let mut elevation_m = tectonics.elevation_m.clone();
        let mut erosion_passes = Vec::with_capacity(EROSION_PASSES);
        for _ in 0..EROSION_PASSES {
            let ocean = climate::ocean_mask(&grid, &elevation_m, &tectonics.oceanic)?;
            let (_, forcing, runoff, hydro) =
                physical_water(definition, &grid, &climate_grid, &elevation_m, &ocean)?;
            hydro.validate(&grid, &elevation_m, &ocean, &runoff.local_m3)?;
            let pass = erosion::erode(
                &grid,
                &elevation_m,
                &tectonics.erodibility,
                &tectonics.cold_phase,
                (&hydro.downstream, &hydro.rank),
                &hydro.discharge_m3,
                &forcing,
            )?;
            pass.validate(&grid, &elevation_m)?;
            elevation_m.clone_from(&pass.elevation_m);
            erosion_passes.push(pass);
        }
        let (ocean, ocean_seeds) =
            climate::ocean_mask_with_seeds(&grid, &elevation_m, &tectonics.oceanic)?;
        let (climate, climate_forcing, runoff, hydrology) =
            physical_water(definition, &grid, &climate_grid, &elevation_m, &ocean)?;
        let region_geology = GeologyContext::build(&grid, &tectonics)?;
        let basin_owner = basin_owners(&hydrology);
        let water_body_owner = region_water::root_water_owners(&grid, &ocean, &hydrology)?;
        let plan = Self::Plane(Box::new(PlanePlan {
            grid,
            climate_grid,
            tectonics,
            region_geology,
            elevation_m,
            ocean,
            ocean_seeds,
            climate,
            climate_forcing,
            runoff,
            hydrology,
            water_body_owner,
            basin_owner,
            erosion_passes,
        }));
        plan.validate(definition)?;
        Ok(plan)
    }

    pub fn basin_fixture(&self) -> Option<&BasinFixture> {
        match self {
            Self::BasinFixture(basin) => Some(basin),
            Self::Plane(_) => None,
        }
    }
    pub fn plane(&self) -> Option<&PlanePlan> {
        match self {
            Self::Plane(plane) => Some(plane),
            Self::BasinFixture(_) => None,
        }
    }

    pub fn validate(&self, definition: &WorldDefinition) -> Result<(), PrepareError> {
        definition.check_preparation_support()?;
        match self {
            Self::BasinFixture(basin) => {
                if definition.input().refinement_profile != "w0-basin" {
                    return Err(PrepareError::Invalid(
                        "fixture root cannot stand in for a plane root",
                    ));
                }
                BasinFixture::from_canonical_bytes(&basin.canonical_bytes())
                    .map_err(PrepareError::Invalid)?;
            }
            Self::Plane(p) => {
                if definition.input().refinement_profile != "earth-root-v1"
                    || p.grid != Grid::from_definition(definition, definition.input().macro_cell_m)?
                    || p.climate_grid
                        != Grid::from_definition(definition, definition.input().climate_cell_m)?
                    || p.elevation_m.len() != p.grid.len()
                    || !p.elevation_m.iter().all(|v| v.is_finite())
                    || p.ocean.len() != p.grid.len()
                    || p.erosion_passes.len() != EROSION_PASSES
                {
                    return Err(PrepareError::Invalid(
                        "root grid/profile/finite elevation mismatch",
                    ));
                }
                p.tectonics.validate(&p.grid)?;
                p.region_geology
                    .validate_provenance(&p.grid, &p.tectonics)?;
                p.climate.validate(&p.climate_grid)?;
                p.climate_forcing.validate(&p.grid)?;
                if p.climate_forcing.ocean != p.ocean {
                    return Err(PrepareError::Invalid(
                        "climate surface mask differs from root ocean",
                    ));
                }
                p.runoff.validate(&p.grid, &p.ocean)?;
                p.hydrology
                    .validate(&p.grid, &p.elevation_m, &p.ocean, &p.runoff.local_m3)?;
                if p.basin_owner != basin_owners(&p.hydrology) {
                    return Err(PrepareError::Invalid("root basin ownership mismatch"));
                }
                if p.water_body_owner
                    != region_water::root_water_owners(&p.grid, &p.ocean, &p.hydrology)?
                {
                    return Err(PrepareError::Invalid("root water body ownership mismatch"));
                }
                let mut previous = &p.tectonics.elevation_m;
                for pass in &p.erosion_passes {
                    pass.validate(&p.grid, previous)?;
                    previous = &pass.elevation_m;
                }
                let (expected_ocean, expected_seeds) =
                    climate::ocean_mask_with_seeds(&p.grid, &p.elevation_m, &p.tectonics.oceanic)?;
                if previous != &p.elevation_m
                    || p.ocean != expected_ocean
                    || p.ocean_seeds != expected_seeds
                {
                    return Err(PrepareError::Invalid(
                        "root erosion/ocean provenance mismatch",
                    ));
                }
            }
        }
        Ok(())
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PrepareError> {
        match self {
            Self::BasinFixture(basin) => Ok(basin.canonical_bytes()),
            Self::Plane(plan) => {
                let mut out = Vec::new();
                out.extend_from_slice(MAGIC);
                out.resize(48, 0);
                serde_json::to_writer(&mut out, plan)
                    .map_err(|_| PrepareError::Invalid("root encode failed"))?;
                let length = (out.len() - 48) as u64;
                let checksum = hash(&out[48..]);
                out[8..16].copy_from_slice(&length.to_le_bytes());
                out[16..48].copy_from_slice(&checksum);
                Ok(out)
            }
        }
    }

    pub fn from_canonical_bytes(
        bytes: &[u8],
        definition: &WorldDefinition,
    ) -> Result<Self, PrepareError> {
        let plan = if definition.input().refinement_profile == "w0-basin" {
            Self::BasinFixture(Box::new(
                BasinFixture::from_canonical_bytes(bytes).map_err(PrepareError::Invalid)?,
            ))
        } else {
            if bytes.len() < 48 || &bytes[..8] != MAGIC {
                return Err(PrepareError::Invalid("invalid root magic/header"));
            }
            let length = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
            if length != (bytes.len() - 48) as u64
                || bytes[16..48] != hash(&bytes[48..])
                || bytes.len() as u64 > definition.estimate()?.root_payload_limit_bytes
            {
                return Err(PrepareError::Invalid(
                    "root length/checksum/budget mismatch",
                ));
            }
            let plane = serde_json::from_slice(&bytes[48..])
                .map_err(|_| PrepareError::Invalid("invalid canonical root payload"))?;
            Self::Plane(Box::new(plane))
        };
        plan.validate(definition)?;
        Ok(plan)
    }
}

fn basin_owners(hydrology: &HydrologyPlan) -> Vec<usize> {
    // Hydrology validation guarantees a permutation of ranks and decreasing downstream rank.
    let n = hydrology.rank.len();
    let mut inverse_rank = vec![0; n];
    for (i, &rank) in hydrology.rank.iter().enumerate() {
        inverse_rank[rank] = i;
    }
    let mut owner = vec![0; n];
    for i in inverse_rank {
        owner[i] = if let Some(depression) = hydrology.depression[i] {
            hydrology.depressions[depression].spill_from
        } else {
            hydrology.downstream[i].map_or(i, |downstream| owner[downstream])
        };
    }
    owner
}

fn physical_water(
    definition: &WorldDefinition,
    grid: &Grid,
    climate_grid: &Grid,
    dem: &[f64],
    ocean: &[bool],
) -> Result<(ClimatePlan, ErosionClimate, RunoffPlan, HydrologyPlan), PrepareError> {
    let mut climate_dem = vec![0.0; climate_grid.len()];
    let mut ocean_area = vec![0.0; climate_grid.len()];
    for (i, &height) in dem.iter().enumerate() {
        for (j, area) in overlaps(grid, i, climate_grid) {
            climate_dem[j] += (if ocean[i] { 0.0 } else { height }) * area / climate_grid.area(j);
            if ocean[i] {
                ocean_area[j] += area;
            }
        }
    }
    // Never transport heat across a sub-grid land barrier: only completely ocean
    // climate cells are connected. Mixed cells receive local ocean influence in W3.
    let climate_ocean: Vec<_> = ocean_area
        .iter()
        .enumerate()
        .map(|(i, &a)| a >= climate_grid.area(i) * (1.0 - 1e-12))
        .collect();
    let d = definition.input();
    let climate = climate::solve(
        climate_grid,
        &climate_dem,
        &climate_ocean,
        d.equator_z_m,
        d.climate_radius_m,
    )?;
    let mut forcing = ErosionClimate {
        temperature_c: vec![[0.0; 4]; grid.len()],
        precipitation_mm: vec![[0.0; 4]; grid.len()],
        potential_et_mm: vec![[0.0; 4]; grid.len()],
        snow_fraction: vec![[0.0; 4]; grid.len()],
        wind_m_s: vec![[[0.0; 2]; 4]; grid.len()],
        ocean: ocean.to_vec(),
    };
    for (i, &ground_height) in dem.iter().enumerate() {
        let height = if ocean[i] { 0.0 } else { ground_height };
        for (j, area) in overlaps(grid, i, climate_grid) {
            let weight = area / grid.area(i);
            let c = &climate.cells[j];
            for s in 0..4 {
                // Physical lapse adjustment; precipitation integral is unchanged.
                forcing.temperature_c[i][s] +=
                    (c.temperature_c[s] - 0.006 * (height - climate_dem[j])) * weight;
                forcing.precipitation_mm[i][s] += c.precipitation_mm[s] * weight;
                forcing.potential_et_mm[i][s] += c.potential_et_mm[s] * weight;
                forcing.snow_fraction[i][s] += c.snow_fraction[s] * weight;
                for axis in 0..2 {
                    forcing.wind_m_s[i][s][axis] += c.wind_m_s[s][axis] * weight;
                }
            }
        }
    }
    let runoff = hydrology::runoff(
        grid,
        &forcing.temperature_c,
        &forcing.precipitation_mm,
        &forcing.potential_et_mm,
        &forcing.snow_fraction,
        ocean,
    )?;
    let losses = hydrology::LakeLosses {
        evaporation_demand_m3: (0..grid.len())
            .map(|i| {
                std::array::from_fn(|s| {
                    (forcing.potential_et_mm[i][s] * 0.001 * grid.area(i)
                        - runoff.actual_et_m3[i][s])
                        .max(0.0)
                })
            })
            .collect(),
        groundwater_export_demand_m3: vec![[0.0; 4]; grid.len()],
    };
    let hydro = hydrology::solve_with_lake_losses(grid, dem, ocean, &runoff.local_m3, &losses)?;
    Ok((climate, forcing, runoff, hydro))
}

/// Actual overlap areas, not nearest-cell copies: both grids tile one domain.
fn overlaps<'a>(
    source: &Grid,
    i: usize,
    target: &'a Grid,
) -> impl Iterator<Item = (usize, f64)> + 'a {
    let (cx, cz) = source.center(i);
    let x0 = source.min_x_m + (i % source.columns) as f64 * source.step_m;
    let z0 = source.min_z_m + (i / source.columns) as f64 * source.step_m;
    let x1 = 2.0 * cx - x0;
    let z1 = 2.0 * cz - z0;
    let first = target.index_at(x0, z0);
    let last = target.index_at(x1 - (x1 - x0) * 1e-12, z1 - (z1 - z0) * 1e-12);
    (first / target.columns..=last / target.columns).flat_map(move |z| {
        (first % target.columns..=last % target.columns).filter_map(move |x| {
            let j = z * target.columns + x;
            let tx0 = target.min_x_m + x as f64 * target.step_m;
            let tz0 = target.min_z_m + z as f64 * target.step_m;
            let tx1 = (tx0 + target.step_m).min(target.min_x_m + target.width_m);
            let tz1 = (tz0 + target.step_m).min(target.min_z_m + target.depth_m);
            let area = (x1.min(tx1) - x0.max(tx0)).max(0.0) * (z1.min(tz1) - z0.max(tz0)).max(0.0);
            (area > 0.0).then_some((j, area))
        })
    })
}

#[cfg(test)]
#[path = "world_plan_tests.rs"]
mod tests;
