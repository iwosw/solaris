//! Discardable, immutable physical quadtree plans. No upstream or sibling preparation.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::time::Instant;

use super::definition::WorldIdentity;
use super::prepared::PrepareError;
use super::region_geology::{self, GeologyEcologySample};
use super::region_water::{self, WaterGeometry, WaterPlan};
use super::world_plan::PlanePlan;

/// A sub-meter live leaf needs only two half-meter interpolation intervals.
pub const LATTICE_SIDE: usize = 3;
pub const LATTICE_POINTS: usize = LATTICE_SIDE * LATTICE_SIDE;
pub const MAX_DEPTH: u8 = 30;
/// Pinned earth-root-v1 detail, not a residency/LOD setting.
pub const PHYSICAL_DETAIL_M: f64 = 1.0;
pub const REGION_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GeoNodeId {
    pub root: u64,
    pub level: u8,
    pub x: u32,
    pub z: u32,
}
impl GeoNodeId {
    pub fn child(self, quadrant: u8) -> Result<Self, PrepareError> {
        if quadrant >= 4 || self.level >= MAX_DEPTH {
            return Err(PrepareError::Invalid("invalid region child/depth"));
        }
        Ok(Self {
            root: self.root,
            level: self.level + 1,
            x: self.x * 2 + u32::from(quadrant & 1),
            z: self.z * 2 + u32::from(quadrant >> 1),
        })
    }
    pub fn seed(self, identity: WorldIdentity, subsystem: &[u8]) -> [u8; 32] {
        let mut h = Sha256::new();
        h.update(b"Solaris-Region-node-v1\0");
        h.update(identity.bytes());
        h.update((subsystem.len() as u64).to_le_bytes());
        h.update(subsystem);
        h.update(self.root.to_le_bytes());
        h.update([self.level]);
        h.update(self.x.to_le_bytes());
        h.update(self.z.to_le_bytes());
        h.finalize().into()
    }
}

/// Extensive local budgets only. A through-going reach is owned once in WaterGeometry,
/// not counted as a new source in every child it crosses.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RegionBudget {
    pub area_m2: f64,
    pub local_runoff_m3: [f64; 4],
    pub reach_evaporation_m3: [f64; 4],
    pub groundwater_export_m3: [f64; 4],
    pub lake_evaporation_m3: [f64; 4],
    pub lake_groundwater_export_m3: [f64; 4],
    /// Physical final-season stocks remain extensive; quantization is not a water source.
    pub lake_storage_m3: [f64; 4],
    pub sediment_source_m3: [f64; 2],
    pub sediment_deposited_m3: [f64; 2],
    pub sediment_stored_m3: [f64; 2],
}
impl RegionBudget {
    pub fn children(self) -> [Self; 4] {
        let q = Self {
            area_m2: self.area_m2 * 0.25,
            local_runoff_m3: self.local_runoff_m3.map(|v| v * 0.25),
            reach_evaporation_m3: self.reach_evaporation_m3.map(|v| v * 0.25),
            groundwater_export_m3: self.groundwater_export_m3.map(|v| v * 0.25),
            lake_evaporation_m3: self.lake_evaporation_m3.map(|v| v * 0.25),
            lake_groundwater_export_m3: self.lake_groundwater_export_m3.map(|v| v * 0.25),
            lake_storage_m3: self.lake_storage_m3.map(|v| v * 0.25),
            sediment_source_m3: self.sediment_source_m3.map(|v| v * 0.25),
            sediment_deposited_m3: self.sediment_deposited_m3.map(|v| v * 0.25),
            sediment_stored_m3: self.sediment_stored_m3.map(|v| v * 0.25),
        };
        [q; 4]
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NaturalColumnSample {
    pub ground_m: f64,
    pub ground_y: i32,
    pub water: WaterPlan,
    pub water_surface_y: Option<i32>,
    pub bed_y: Option<i32>,
    /// Rendered bed shift in block Y, never added to physical m³/flux budgets.
    pub bed_quantization_residual_y: i32,
    pub geology_ecology: GeologyEcologySample,
    pub protected: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryContract {
    /// Geometry-only canonical owner: lowest incident same-level node address.
    pub owner: GeoNodeId,
    pub key: [u8; 32],
    pub positions_m: Vec<[f64; 2]>,
    pub samples: Vec<NaturalColumnSample>,
    pub gradients: Vec<[f64; 2]>,
    /// The root basin/divide assignments may not be changed by detail.
    pub basin_cells: Vec<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RegionPlan {
    pub identity: WorldIdentity,
    pub version: u32,
    pub node: GeoNodeId,
    pub bounds_m: [f64; 4],
    pub budget: RegionBudget,
    pub edges: Vec<BoundaryContract>,
    pub corners: Vec<BoundaryContract>,
    pub columns: Vec<NaturalColumnSample>,
    /// Uncarved reconstruction; ready sampling applies protected water geometry once.
    pub surface_m: Vec<f64>,
    pub water_geometry: WaterGeometry,
}
#[derive(Debug, Clone, Copy, Serialize)]
pub struct RegionCostBound {
    pub max_nodes: u32,
    pub lattice_points_per_node: usize,
    pub max_candidate_cells_per_node: usize,
    pub column_evaluations_per_node: usize,
    pub water_evaluations_per_node: usize,
    pub max_features_per_node: usize,
    pub max_arc_segments_per_reach: usize,
    pub surface_passes: u32,
    pub max_record_bytes: usize,
}
#[derive(Debug, Clone, Serialize)]
pub struct ColdPathProfile {
    pub bound: RegionCostBound,
    pub prepared_nodes: u32,
    pub sampled_points: usize,
    pub candidate_cells: usize,
    pub feature_count: usize,
    pub upstream_traversals: u32,
    pub per_node_micros: Vec<u128>,
    pub elapsed_micros: u128,
}

pub struct RegionPlanner<'a> {
    root: &'a PlanePlan,
    identity: WorldIdentity,
    relief_phase: f64,
}
impl<'a> RegionPlanner<'a> {
    pub fn new(root: &'a PlanePlan, identity: WorldIdentity) -> Self {
        let seed = GeoNodeId {
            root: 0,
            level: 0,
            x: 0,
            z: 0,
        }
        .seed(identity, b"constrained-relief");
        let relief_phase = f64::from(u32::from_le_bytes(seed[..4].try_into().unwrap()))
            / f64::from(u32::MAX)
            * std::f64::consts::TAU;
        Self {
            root,
            identity,
            relief_phase,
        }
    }
    pub fn leaf_at(&self, x_m: f64, z_m: f64) -> Result<GeoNodeId, PrepareError> {
        let g = &self.root.grid;
        if !x_m.is_finite()
            || !z_m.is_finite()
            || x_m < g.min_x_m
            || z_m < g.min_z_m
            || x_m >= g.min_x_m + g.width_m
            || z_m >= g.min_z_m + g.depth_m
        {
            return Err(PrepareError::Invalid(
                "region coordinate outside physical domain",
            ));
        }
        let root = g.index_at(x_m, z_m);
        let b = self.bounds(GeoNodeId {
            root: root as u64,
            level: 0,
            x: 0,
            z: 0,
        })?;
        // Uniform nominal depth also for partial edge cells: no adaptive T-junctions.
        let level = (g.step_m / PHYSICAL_DETAIL_M).log2().ceil().max(0.0) as u8;
        if level > MAX_DEPTH {
            return Err(PrepareError::Invalid(
                "region depth exceeds pinned cost bound",
            ));
        }
        let n = f64::from(1u32 << level);
        Ok(GeoNodeId {
            root: root as u64,
            level,
            x: (((x_m - b[0]) / (b[1] - b[0]) * n).floor() as u32).min((1u32 << level) - 1),
            z: (((z_m - b[2]) / (b[3] - b[2]) * n).floor() as u32).min((1u32 << level) - 1),
        })
    }
    pub fn bounds(&self, node: GeoNodeId) -> Result<[f64; 4], PrepareError> {
        let g = &self.root.grid;
        if node.root >= g.len() as u64
            || node.level > MAX_DEPTH
            || node.x >= (1u32 << node.level)
            || node.z >= (1u32 << node.level)
        {
            return Err(PrepareError::Invalid("invalid region address"));
        }
        let i = node.root as usize;
        let x = i % g.columns;
        let z = i / g.columns;
        let minx = g.min_x_m + x as f64 * g.step_m;
        let minz = g.min_z_m + z as f64 * g.step_m;
        let w = g.step_m.min(g.min_x_m + g.width_m - minx);
        let d = g.step_m.min(g.min_z_m + g.depth_m - minz);
        let physical_depth = (g.step_m / PHYSICAL_DETAIL_M).log2().ceil().max(0.0);
        if f64::from(node.level) > physical_depth {
            return Err(PrepareError::Invalid(
                "region address exceeds pinned physical detail",
            ));
        }
        let n = f64::from(1u32 << node.level);
        let bounds = [
            minx + w * f64::from(node.x) / n,
            minx + w * f64::from(node.x + 1) / n,
            minz + d * f64::from(node.z) / n,
            minz + d * f64::from(node.z + 1) / n,
        ];
        if bounds[1] <= bounds[0] || bounds[3] <= bounds[2] {
            return Err(PrepareError::Invalid(
                "physical refinement loses floating precision",
            ));
        }
        Ok(bounds)
    }
    pub fn cost_bound(&self, node: GeoNodeId) -> Result<RegionCostBound, PrepareError> {
        self.bounds(node)?;
        Ok(RegionCostBound {
            max_nodes: u32::from(node.level) + 1,
            lattice_points_per_node: LATTICE_POINTS,
            max_candidate_cells_per_node: region_water::MAX_CANDIDATE_CELLS,
            column_evaluations_per_node: LATTICE_POINTS + 4 * LATTICE_SIDE + 4,
            water_evaluations_per_node: LATTICE_POINTS + 5 * (4 * LATTICE_SIDE + 4),
            max_features_per_node: region_water::MAX_REACH_FEATURES
                + region_water::MAX_CANDIDATE_CELLS,
            max_arc_segments_per_reach: 16,
            surface_passes: 1,
            max_record_bytes: 4 * 1024 * 1024,
        })
    }
    /// Cold means no derived-cache reads. The immutable root is already admitted/loaded.
    pub fn prepare_cold(
        &self,
        node: GeoNodeId,
    ) -> Result<(RegionPlan, ColdPathProfile), PrepareError> {
        let start = Instant::now();
        let bound = self.cost_bound(node)?;
        let mut profile = ColdPathProfile {
            bound,
            prepared_nodes: 0,
            sampled_points: 0,
            candidate_cells: 0,
            feature_count: 0,
            upstream_traversals: 0,
            per_node_micros: Vec::with_capacity(bound.max_nodes as usize),
            elapsed_micros: 0,
        };
        let mut budget = self.root_budget(node.root as usize);
        let mut final_plan = None;
        for level in 0..=node.level {
            let shift = node.level - level;
            let ancestor = GeoNodeId {
                root: node.root,
                level,
                x: node.x >> shift,
                z: node.z >> shift,
            };
            if level > 0 {
                let q = ((ancestor.z & 1) << 1) | (ancestor.x & 1);
                budget = budget.children()[q as usize];
            }
            let tick = Instant::now();
            let plan = self.prepare_node(ancestor, budget)?;
            profile.prepared_nodes += 1;
            profile.sampled_points += bound.column_evaluations_per_node;
            profile.candidate_cells += plan.water_geometry.candidate_cells;
            profile.feature_count += plan.water_geometry.feature_count();
            profile.per_node_micros.push(tick.elapsed().as_micros());
            final_plan = Some(plan);
        }
        profile.elapsed_micros = start.elapsed().as_micros();
        Ok((final_plan.expect("inclusive ancestor path"), profile))
    }
    pub fn prepare(&self, node: GeoNodeId) -> Result<RegionPlan, PrepareError> {
        self.bounds(node)?;
        let mut budget = self.root_budget(node.root as usize);
        for _ in 0..node.level {
            budget = budget.children()[0];
        }
        self.prepare_node(node, budget)
    }
    fn root_budget(&self, i: usize) -> RegionBudget {
        let h = &self.root.hydrology;
        let e = self
            .root
            .erosion_passes
            .last()
            .expect("validated physical root erosion");
        RegionBudget {
            area_m2: self.root.grid.area(i),
            local_runoff_m3: self.root.runoff.local_m3[i],
            reach_evaporation_m3: h.evaporation_m3[i],
            groundwater_export_m3: h.groundwater_export_m3[i],
            lake_evaporation_m3: h.lake_evaporation_m3[i],
            lake_groundwater_export_m3: h.lake_groundwater_export_m3[i],
            lake_storage_m3: h.seasonal_water_level_m[i].map(|level| {
                if self.root.ocean[i] {
                    0.0
                } else {
                    level.map_or(0.0, |v| {
                        (v - self.root.elevation_m[i]).max(0.0) * self.root.grid.area(i)
                    })
                }
            }),
            sediment_source_m3: e.source_m3[i],
            sediment_deposited_m3: e.deposited_m3[i],
            sediment_stored_m3: e.stored_m3[i],
        }
    }
    fn prepare_node(
        &self,
        node: GeoNodeId,
        budget: RegionBudget,
    ) -> Result<RegionPlan, PrepareError> {
        let b = self.bounds(node)?;
        let water = region_water::prepare(self.root, self.identity, b)?;
        let mut columns = Vec::with_capacity(LATTICE_POINTS);
        let mut surface_m = Vec::with_capacity(LATTICE_POINTS);
        for z in 0..LATTICE_SIDE {
            for x in 0..LATTICE_SIDE {
                let p = position(b, x, z);
                let base = self.surface(p[0], p[1]);
                surface_m.push(base);
                columns.push(self.column_from_surface(&water, p[0], p[1], base));
            }
        }
        let mut edges = Vec::with_capacity(4);
        for side in 0..4 {
            let positions = (0..LATTICE_SIDE)
                .map(|i| match side {
                    0 => position(b, 0, i),
                    1 => position(b, LATTICE_SIDE - 1, i),
                    2 => position(b, i, 0),
                    _ => position(b, i, LATTICE_SIDE - 1),
                })
                .collect::<Vec<_>>();
            edges.push(self.contract(node.level, positions, &water));
        }
        let corners = [[b[0], b[2]], [b[1], b[2]], [b[0], b[3]], [b[1], b[3]]]
            .into_iter()
            .map(|p| self.contract(node.level, vec![p], &water))
            .collect();
        Ok(RegionPlan {
            identity: self.identity,
            version: REGION_VERSION,
            node,
            bounds_m: b,
            budget,
            edges,
            corners,
            columns,
            surface_m,
            water_geometry: water,
        })
    }
    fn column(&self, water: &WaterGeometry, x: f64, z: f64) -> NaturalColumnSample {
        self.column_from_surface(water, x, z, self.surface(x, z))
    }
    fn column_from_surface(
        &self,
        water: &WaterGeometry,
        x: f64,
        z: f64,
        base: f64,
    ) -> NaturalColumnSample {
        let wet = region_water::sample(water, x, z, base);
        let (surface, bed, bed_quantization_residual_y) = rendered_water(&wet.water);
        let ground_y = bed.map_or(compact_y(wet.ground_m), |b| compact_y(wet.ground_m).min(b));
        let mut column = NaturalColumnSample {
            ground_m: wet.ground_m,
            ground_y,
            geology_ecology: region_geology::sample(
                self.root,
                x,
                z,
                wet.ground_m,
                surface.is_some(),
            ),
            water: wet.water,
            water_surface_y: surface,
            bed_y: bed,
            bed_quantization_residual_y,
            protected: wet.protected,
        };
        finalize_water_ecology(&mut column);
        column
    }
    fn contract(
        &self,
        level: u8,
        positions: Vec<[f64; 2]>,
        water: &WaterGeometry,
    ) -> BoundaryContract {
        let first = positions[0];
        let last = *positions.last().unwrap();
        let owner = self.owner(level, first);
        let mut h = Sha256::new();
        h.update(owner.seed(self.identity, b"boundary"));
        for p in [first, last] {
            for v in p {
                h.update(v.to_bits().to_le_bytes());
            }
        }
        let samples = positions
            .iter()
            .map(|p| self.column(water, p[0], p[1]))
            .collect();
        let gradients = positions
            .iter()
            .map(|p| self.gradient(water, p[0], p[1]))
            .collect();
        let basin_cells = positions
            .iter()
            .map(|p| self.root.basin_owner[self.root.grid.index_at(p[0], p[1])] as u64)
            .collect();
        BoundaryContract {
            owner,
            key: h.finalize().into(),
            positions_m: positions,
            samples,
            gradients,
            basin_cells,
        }
    }
    fn owner(&self, level: u8, p: [f64; 2]) -> GeoNodeId {
        let g = &self.root.grid;
        // Select NW incident geometry. Never request the selected neighbor's plan.
        let root = g.index_at(p[0].next_down(), p[1].next_down());
        let b = self
            .bounds(GeoNodeId {
                root: root as u64,
                level: 0,
                x: 0,
                z: 0,
            })
            .unwrap();
        let n = 1u32 << level;
        GeoNodeId {
            root: root as u64,
            level,
            x: (((p[0] - b[0]) / (b[1] - b[0]) * f64::from(n))
                .next_down()
                .floor()
                .max(0.0) as u32)
                .min(n - 1),
            z: (((p[1] - b[2]) / (b[3] - b[2]) * f64::from(n))
                .next_down()
                .floor()
                .max(0.0) as u32)
                .min(n - 1),
        }
    }
    fn gradient(&self, water: &WaterGeometry, x: f64, z: f64) -> [f64; 2] {
        let e = self.root.grid.step_m.min(1.0) / 1024.0;
        let height = |x, z| region_water::sample(water, x, z, self.surface(x, z)).ground_m;
        [
            (height(x + e, z) - height(x - e, z)) / (2.0 * e),
            (height(x, z + e) - height(x, z - e)) / (2.0 * e),
        ]
    }
    fn surface(&self, x: f64, z: f64) -> f64 {
        let g = &self.root.grid;
        let i = g.index_at(x, z);
        let center = g.center(i);
        let ix = i % g.columns;
        let iz = i / g.columns;
        let jx = if x < center.0 {
            ix.saturating_sub(1)
        } else {
            (ix + 1).min(g.columns - 1)
        };
        let jz = if z < center.1 {
            iz.saturating_sub(1)
        } else {
            (iz + 1).min(g.rows - 1)
        };
        let a = iz * g.columns + jx;
        let b = jz * g.columns + ix;
        let c = jz * g.columns + jx;
        let tx = if jx == ix {
            0.0
        } else {
            ((x - center.0) / (g.center(a).0 - center.0)).clamp(0.0, 1.0)
        };
        let tz = if jz == iz {
            0.0
        } else {
            ((z - center.1) / (g.center(b).1 - center.1)).clamp(0.0, 1.0)
        };
        let mix = |u: f64, v: f64, t: f64| u + (v - u) * t;
        let base = mix(
            mix(self.root.elevation_m[i], self.root.elevation_m[a], tx),
            mix(self.root.elevation_m[b], self.root.elevation_m[c], tx),
            tz,
        );
        // Detail fades before any wet/dry root interface and never changes its sign.
        let strength = mix(
            mix(
                self.root.tectonics.erodibility[i],
                self.root.tectonics.erodibility[a],
                tx,
            ),
            mix(
                self.root.tectonics.erodibility[b],
                self.root.tectonics.erodibility[c],
                tx,
            ),
            tz,
        );
        let mut amplitude = base.abs().min(8.0) * 0.125 / (1.0 + strength.max(0.0));
        if [i, a, b, c].into_iter().any(|k| {
            self.root.ocean[k] != self.root.ocean[i]
                || self.root.hydrology.depression[k] != self.root.hydrology.depression[i]
                || self.root.basin_owner[k] != self.root.basin_owner[i]
        }) {
            amplitude = 0.0;
        }
        let phase = self.relief_phase;
        base + amplitude
            * ((x / 73.0 + phase).sin() * (z / 97.0 - phase).sin()
                + 0.25 * (x / 19.0 + phase).sin() * (z / 23.0).sin())
    }
}
fn position(b: [f64; 4], x: usize, z: usize) -> [f64; 2] {
    [
        b[0] + (b[1] - b[0]) * x as f64 / (LATTICE_SIDE - 1) as f64,
        b[2] + (b[3] - b[2]) * z as f64 / (LATTICE_SIDE - 1) as f64,
    ]
}
pub fn compact_y(h_m: f64) -> i32 {
    if h_m >= 0.0 {
        63 + (224.0 * (h_m / 3000.0).tanh()).round() as i32
    } else {
        63 + (111.0 * (h_m / 4000.0).tanh()).round() as i32
    }
}
fn rendered_water(water: &WaterPlan) -> (Option<i32>, Option<i32>, i32) {
    match water {
        WaterPlan::None => (None, None, 0),
        WaterPlan::Ocean {
            surface_m, bed_m, ..
        }
        | WaterPlan::Lake {
            surface_m, bed_m, ..
        }
        | WaterPlan::River {
            surface_m, bed_m, ..
        } => {
            let surface = compact_y(*surface_m);
            // Feature-owned bed-only quantization correction. Water level/Q never rises.
            // Exact zero physical depth remains dry (the footprint sampler excludes it).
            let raw_bed = compact_y(*bed_m);
            let bed = raw_bed.min(surface - 1);
            (Some(surface), Some(bed), bed - raw_bed)
        }
    }
}

impl RegionPlan {
    /// Sampling a ready plan is bounded CPU-only; no planner, disk or global context.
    pub fn sample(&self, x_m: f64, z_m: f64) -> Result<NaturalColumnSample, PrepareError> {
        let b = self.bounds_m;
        if !x_m.is_finite()
            || !z_m.is_finite()
            || x_m < b[0]
            || x_m > b[1]
            || z_m < b[2]
            || z_m > b[3]
        {
            return Err(PrepareError::Invalid("sample outside prepared region"));
        }
        let fx = (x_m - b[0]) / (b[1] - b[0]) * (LATTICE_SIDE - 1) as f64;
        let fz = (z_m - b[2]) / (b[3] - b[2]) * (LATTICE_SIDE - 1) as f64;
        let x = (fx.floor() as usize).min(LATTICE_SIDE - 2);
        let z = (fz.floor() as usize).min(LATTICE_SIDE - 2);
        let tx = fx - x as f64;
        let tz = fz - z as f64;
        if fx == fx.round() && fz == fz.round() {
            return Ok(self.columns[fz as usize * LATTICE_SIDE + fx as usize].clone());
        }
        let indices = [
            z * LATTICE_SIDE + x,
            z * LATTICE_SIDE + x + 1,
            (z + 1) * LATTICE_SIDE + x,
            (z + 1) * LATTICE_SIDE + x + 1,
        ];
        let weights = [
            (1.0 - tx) * (1.0 - tz),
            tx * (1.0 - tz),
            (1.0 - tx) * tz,
            tx * tz,
        ];
        let ground = indices
            .into_iter()
            .zip(weights)
            .map(|(i, w)| self.surface_m[i] * w)
            .sum();
        let mut column = self.columns[((fz.round() as usize).min(LATTICE_SIDE - 1)) * LATTICE_SIDE
            + (fx.round() as usize).min(LATTICE_SIDE - 1)]
        .clone();
        let water = region_water::sample(&self.water_geometry, x_m, z_m, ground);
        column.ground_m = water.ground_m;
        column.ground_y = compact_y(water.ground_m);
        column.protected = water.protected;
        let (surface, bed, residual) = rendered_water(&water.water);
        column.ground_y = bed.map_or(column.ground_y, |b| column.ground_y.min(b));
        column.water = water.water;
        column.water_surface_y = surface;
        column.bed_y = bed;
        column.bed_quantization_residual_y = residual;
        reconstruct_inputs(indices.map(|i| &self.columns[i]), weights, &mut column);
        Ok(column)
    }
}
fn reconstruct_inputs(
    corners: [&NaturalColumnSample; 4],
    weights: [f64; 4],
    column: &mut NaturalColumnSample,
) {
    let blend = |values: [f64; 4]| {
        values
            .into_iter()
            .zip(weights)
            .map(|(v, w)| v * w)
            .sum::<f64>()
    };
    let result = &mut column.geology_ecology;
    for (i, band) in result.geology.bands.iter_mut().enumerate() {
        band.age_ma = blend(corners.map(|c| c.geology_ecology.geology.bands[i].age_ma));
        band.permeability_m_s =
            blend(corners.map(|c| c.geology_ecology.geology.bands[i].permeability_m_s));
        band.solubility = blend(corners.map(|c| c.geology_ecology.geology.bands[i].solubility));
        band.erodibility = blend(corners.map(|c| c.geology_ecology.geology.bands[i].erodibility));
    }
    let soil_depth = blend(corners.map(|c| {
        c.geology_ecology.ecology.soil_depth_m
            / if c.geology_ecology.ecology.wet {
                0.25
            } else {
                1.0
            }
    })) * if column.water_surface_y.is_some() {
        0.25
    } else {
        1.0
    };
    let soil_bottom = column.ground_m - soil_depth;
    let deposit_depth = blend(corners.map(|c| {
        c.geology_ecology.geology.bands[3].top_m - c.geology_ecology.geology.bands[3].bottom_m
    }));
    let unconsolidated_bottom = soil_bottom - deposit_depth;
    let basement_top =
        blend(corners.map(|c| c.geology_ecology.geology.bands[0].top_m)).min(unconsolidated_bottom);
    let middle = blend(corners.map(|c| c.geology_ecology.geology.bands[1].top_m))
        .min(unconsolidated_bottom)
        .max(basement_top);
    let interfaces = [
        blend(corners.map(|c| c.geology_ecology.geology.bands[0].bottom_m)).min(basement_top),
        basement_top,
        middle,
        unconsolidated_bottom,
        soil_bottom,
        column.ground_m,
    ];
    for (i, band) in result.geology.bands.iter_mut().enumerate() {
        band.bottom_m = interfaces[i];
        band.top_m = interfaces[i + 1];
    }
    result.geology.folding = blend(corners.map(|c| c.geology_ecology.geology.folding));
    let sine = blend(corners.map(|c| (2.0 * c.geology_ecology.geology.fold_axis_rad).sin()));
    let cosine = blend(corners.map(|c| (2.0 * c.geology_ecology.geology.fold_axis_rad).cos()));
    result.geology.fold_axis_rad = sine.atan2(cosine) * 0.5;
    result.geology.fault_displacement_m =
        blend(corners.map(|c| c.geology_ecology.geology.fault_displacement_m));
    result.geology.intrusion_fraction =
        blend(corners.map(|c| c.geology_ecology.geology.intrusion_fraction));
    result.geology.mineralization =
        blend(corners.map(|c| c.geology_ecology.geology.mineralization));
    result.geology.cold_phase_fraction =
        blend(corners.map(|c| c.geology_ecology.geology.cold_phase_fraction));

    // IDs are never interpolated. Merge weighted root provenance, with bounded
    // stack storage; each lattice interval has at most four nonzero root corners.
    let mut provenance: [Option<region_geology::GeologyProvenance>; 16] =
        std::array::from_fn(|_| None);
    let mut count = 0;
    for (corner, weight) in corners.into_iter().zip(weights) {
        for source in &corner.geology_ecology.geology.provenance {
            let contribution = source.weight * weight;
            if contribution <= 0.0 {
                continue;
            }
            if let Some(existing) = provenance[..count]
                .iter_mut()
                .flatten()
                .find(|p| p.root_cell == source.root_cell)
            {
                existing.weight += contribution;
            } else {
                let mut source = source.clone();
                source.weight = contribution;
                provenance[count] = Some(source);
                count += 1;
            }
        }
    }
    provenance[..count].sort_unstable_by(|a, b| {
        let a = a.as_ref().unwrap();
        let b = b.as_ref().unwrap();
        b.weight
            .total_cmp(&a.weight)
            .then_with(|| a.root_cell.cmp(&b.root_cell))
    });
    let retained = provenance[..count.min(4)]
        .iter()
        .flatten()
        .map(|p| p.weight)
        .sum::<f64>();
    for (i, target) in result.geology.provenance.iter_mut().enumerate() {
        if i < count.min(4) {
            *target = provenance[i].as_ref().unwrap().clone();
            target.weight /= retained;
        } else {
            target.weight = 0.0;
        }
    }
    let ecology = &mut result.ecology;
    for season in 0..4 {
        ecology.temperature_c[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.temperature_c[season]));
        ecology.precipitation_mm[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.precipitation_mm[season]));
        ecology.potential_et_mm[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.potential_et_mm[season]));
        ecology.moisture_fraction[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.moisture_fraction[season]));
        ecology.snow_water_mm[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.snow_water_mm[season]));
        ecology.snow_fraction[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.snow_fraction[season]));
        ecology.water_deficit_mm[season] =
            blend(corners.map(|c| c.geology_ecology.ecology.water_deficit_mm[season]));
    }
    ecology.slope = blend(corners.map(|c| c.geology_ecology.ecology.slope));
    ecology.ocean_fraction = blend(corners.map(|c| c.geology_ecology.ecology.ocean_fraction));
    ecology.shore_fraction = 4.0 * ecology.ocean_fraction * (1.0 - ecology.ocean_fraction);
    ecology.soil_depth_m = soil_depth;
    ecology.rock_exposure = if ecology.soil_depth_m <= 0.0 {
        1.0
    } else {
        blend(corners.map(|c| c.geology_ecology.ecology.rock_exposure))
    };
    ecology.hydroperiod_fraction =
        blend(corners.map(|c| c.geology_ecology.ecology.hydroperiod_fraction));
    finalize_water_ecology(column);
}

fn finalize_water_ecology(column: &mut NaturalColumnSample) {
    let ecology = &mut column.geology_ecology.ecology;
    ecology.wet = column.water_surface_y.is_some();
    match &column.water {
        WaterPlan::None => {}
        WaterPlan::Ocean { hydroperiod, .. }
        | WaterPlan::Lake { hydroperiod, .. }
        | WaterPlan::River { hydroperiod, .. } => {
            for (season, wet) in hydroperiod.wet_seasons.iter().enumerate() {
                if *wet {
                    ecology.moisture_fraction[season] = 1.0;
                    ecology.water_deficit_mm[season] = 0.0;
                }
            }
            ecology.hydroperiod_fraction =
                hydroperiod.wet_seasons.iter().filter(|wet| **wet).count() as f64 * 0.25;
        }
    }
}

#[cfg(test)]
#[path = "region_tests.rs"]
mod tests;
