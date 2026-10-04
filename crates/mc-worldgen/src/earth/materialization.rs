//! Immutable, activation-free compact base blocks from checked root/region plans.
//! Preparation may solve regions off-thread. Ready sampling never solves, reads
//! storage, consults gameplay state, or allocates.
use mc_data::Identifier;
use mc_world::{BlockRegistry, BlockStateId, Chunk, ChunkPos, ChunkSection, MAX_Y, MIN_Y};
use std::sync::Arc;

use super::definition::{WorldDefinition, WorldIdentity};
use super::prepared::PrepareError;
use super::region::{GeoNodeId, NaturalColumnSample, RegionPlan, RegionPlanner, compact_y};
use super::region_geology::RockClass;
use super::world_plan::WorldPlan;

/// A compact chunk has 256 columns and needs at most one leaf prepare per column.
/// Only one RegionPlan is resident in this materializer's preparation scratch.
pub const MAX_REGION_PREPARES_PER_CHUNK: usize = 256 + 16;

#[derive(Clone, Copy)]
pub(crate) struct Palette {
    air: BlockStateId,
    water: BlockStateId,
    rocks: [BlockStateId; 10],
    grass: BlockStateId,
    sand: BlockStateId,
    snow: BlockStateId,
    podzol: BlockStateId,
    log: [BlockStateId; 2],
    leaves: [[BlockStateId; 7]; 2],
    ores: [BlockStateId; 8],
}
impl Palette {
    pub(crate) fn new(blocks: &BlockRegistry) -> Result<Self, PrepareError> {
        let state = |name: &str| {
            let name = Identifier::parse(name)
                .map_err(|_| PrepareError::Invalid("invalid materializer block identifier"))?;
            blocks
                .block(&name)
                .map(|b| b.default)
                .ok_or(PrepareError::Invalid("missing materializer block state"))
        };
        let leaves = |name: &str| -> Result<[BlockStateId; 7], PrepareError> {
            let id = Identifier::parse(name)
                .map_err(|_| PrepareError::Invalid("invalid leaf identifier"))?;
            let mut states = [state(name)?; 7];
            for (index, leaf) in states.iter_mut().enumerate() {
                *leaf = blocks
                    .by_name_and_props(
                        &id,
                        &[
                            ("distance".into(), (index + 1).to_string()),
                            ("persistent".into(), "false".into()),
                            ("waterlogged".into(), "false".into()),
                        ],
                    )
                    .ok_or(PrepareError::Invalid(
                        "missing generated leaf distance state",
                    ))?;
            }
            Ok(states)
        };
        Ok(Self {
            air: state("minecraft:air")?,
            water: state("minecraft:water")?,
            grass: state("minecraft:grass_block")?,
            sand: state("minecraft:sand")?,
            snow: state("minecraft:snow_block")?,
            podzol: state("minecraft:podzol")?,
            log: [state("minecraft:oak_log")?, state("minecraft:spruce_log")?],
            leaves: [
                leaves("minecraft:oak_leaves")?,
                leaves("minecraft:spruce_leaves")?,
            ],
            ores: [
                state("minecraft:coal_ore")?,
                state("minecraft:iron_ore")?,
                state("minecraft:copper_ore")?,
                state("minecraft:gold_ore")?,
                state("minecraft:diamond_ore")?,
                state("minecraft:redstone_ore")?,
                state("minecraft:lapis_ore")?,
                state("minecraft:emerald_ore")?,
            ],
            // Compact rendering of W3 physical lithologies. These mappings and
            // quantization rules are pinned by the algorithm artifact, not cache tuning.
            rocks: [
                state("minecraft:basalt")?,
                state("minecraft:granite")?,
                state("minecraft:diorite")?,
                state("minecraft:deepslate")?,
                state("minecraft:calcite")?,
                state("minecraft:sandstone")?,
                state("minecraft:stone")?,
                state("minecraft:gravel")?,
                state("minecraft:clay")?,
                state("minecraft:dirt")?,
            ],
        })
    }
    fn rock(self, class: RockClass) -> BlockStateId {
        self.rocks[match class {
            RockClass::Basalt => 0,
            RockClass::Granite => 1,
            RockClass::Intrusive => 2,
            RockClass::Metamorphic => 3,
            RockClass::Carbonate => 4,
            RockClass::Sandstone => 5,
            RockClass::Shale => 6,
            RockClass::CoarseSediment => 7,
            RockClass::FineSediment => 8,
            RockClass::Soil => 9,
        }]
    }
}

#[derive(Clone, Copy)]
struct RenderedBand {
    bottom_y: i32,
    top_y: i32,
    state: BlockStateId,
    present: bool,
    solubility: f64,
}
#[derive(Clone, Copy)]
struct RenderedColumn {
    ground_y: i32,
    water_y: Option<i32>,
    protected: bool,
    bands: [RenderedBand; 5],
    surface: BlockStateId,
    soil: bool,
    karst_conduit: f64,
    mineralization: f64,
    intrusion: f64,
    world_x: i64,
    world_z: i64,
    seed: u64,
}
impl RenderedColumn {
    fn from_plan(
        column: &NaturalColumnSample,
        palette: Palette,
        x: i64,
        z: i64,
        seed: u64,
    ) -> Self {
        Self {
            ground_y: column.ground_y,
            water_y: column.water_surface_y,
            protected: column.protected,
            surface: surface_state(column, palette),
            soil: column.geology_ecology.ecology.soil_depth_m > 0.25
                && column.water_surface_y.is_none(),
            karst_conduit: (x as f64 / 31.0
                + (seed & 0xFFFF_FFFF) as f64 / u32::MAX as f64 * std::f64::consts::TAU)
                .sin()
                + (z as f64 / 43.0).sin(),
            mineralization: column.geology_ecology.geology.mineralization,
            intrusion: column.geology_ecology.geology.intrusion_fraction,
            world_x: x,
            world_z: z,
            seed,
            bands: std::array::from_fn(|index| {
                let band = &column.geology_ecology.geology.bands[index];
                RenderedBand {
                    bottom_y: compact_y(band.bottom_m),
                    top_y: compact_y(band.top_m),
                    solubility: band.solubility,
                    state: palette.rock(band.lithology),
                    present: band.top_m > band.bottom_m,
                }
            }),
        }
    }
    fn sample(&self, y: i32, palette: &Palette) -> BlockStateId {
        if y > self.ground_y {
            return if self.water_y.is_some_and(|surface| y <= surface) {
                palette.water
            } else {
                palette.air
            };
        }
        if y == self.ground_y {
            return self.surface;
        }
        if self.soil && y >= self.ground_y - 3 {
            return palette.rocks[9];
        }
        // The highest nonempty geological band owns shared quantized interfaces.
        // The basement extends to the bottom of the compact rendering envelope.
        let host = self
            .bands
            .iter()
            .rev()
            .find(|band| band.present && y > band.bottom_y && y <= band.top_y)
            .unwrap_or(&self.bands[0]);
        let rock = host.state;
        // Karst belongs to soluble host strata, not merely any soluble cap above.
        // Protected water columns never acquire a worldgen leak.
        if self.water_y.is_none()
            && !self.protected
            && y < self.ground_y - 12
            && y > MIN_Y + 8
            && host.solubility > 0.25
            && self.karst_conduit.abs() < host.solubility * 0.18
            && y.rem_euclid(9) < 2
        {
            return palette.air;
        }
        if y > MIN_Y + 1 && y < self.ground_y - 6 {
            // Deposits follow host strata and mapped intrusion/mineralization.
            let deposit = feature_hash(
                self.seed,
                self.world_x.div_euclid(4),
                i64::from(y).div_euclid(4),
                self.world_z.div_euclid(4),
            );
            let roll = deposit % 1000;
            if rock == palette.rocks[6] && roll < 45 {
                return palette.ores[0];
            }
            if self.mineralization > 0.05 && roll < (self.mineralization * 70.0) as u64 {
                return palette.ores[if self.intrusion > 0.2 { 2 } else { 1 }];
            }
            if self.intrusion > 0.35 && y < 16 && roll < 8 {
                return palette.ores[3];
            }
            if rock == palette.rocks[3] && y < -32 && roll < 5 {
                return palette.ores[4];
            }
            if self.intrusion > 0.1 && rock == palette.rocks[1] && y < 0 && (8..18).contains(&roll)
            {
                return palette.ores[5];
            }
            if rock == palette.rocks[4] && y < 32 && roll < 12 {
                return palette.ores[6];
            }
            if self.mineralization > 0.2 && rock == palette.rocks[3] && (32..48).contains(&roll) {
                return palette.ores[7];
            }
        }
        rock
    }
}

enum ReadyBase {
    Basin(Box<Chunk>),
    Plane(Box<[Option<RenderedColumn>; 256]>),
}

/// Ready CPU-only handle for a single immutable compact chunk. It owns no store,
/// planner, registry, lease, entity/loot initializer, or mutable cache authority.
pub struct PreparedBaseChunk {
    position: ChunkPos,
    identity: WorldIdentity,
    palette: Palette,
    base: ReadyBase,
    trees: Vec<Tree>,
}
impl PreparedBaseChunk {
    pub fn position(&self) -> ChunkPos {
        self.position
    }
    pub fn identity(&self) -> WorldIdentity {
        self.identity
    }

    pub fn sample(&self, x: u8, y: i32, z: u8) -> Option<BlockStateId> {
        if x >= 16 || z >= 16 || !(MIN_Y..MAX_Y).contains(&y) {
            return None;
        }
        match &self.base {
            ReadyBase::Basin(chunk) => chunk.get_block(x, y, z),
            ReadyBase::Plane(columns) => columns[usize::from(z) * 16 + usize::from(x)]
                .as_ref()
                .map(|column| column.sample(y, &self.palette)),
        }
        .map(|state| {
            if state != self.palette.air {
                return state;
            }
            let world_x = i64::from(self.position.x) * 16 + i64::from(x);
            let world_z = i64::from(self.position.z) * 16 + i64::from(z);
            let mut canopy: Option<(usize, usize)> = None;
            for tree in &self.trees {
                let dx = (world_x - tree.x).abs();
                let dz = (world_z - tree.z).abs();
                let leaf_distance = (dx + dz + i64::from((y - tree.ground_y - tree.height).max(0)))
                    .clamp(1, 7) as usize;
                if dx == 0 && dz == 0 && y > tree.ground_y && y <= tree.ground_y + tree.height {
                    return self.palette.log[tree.kind];
                }
                if dx <= 2
                    && dz <= 2
                    && dx + dz <= 3
                    && (tree.ground_y + tree.height - 2..=tree.ground_y + tree.height + 1)
                        .contains(&y)
                    && canopy.is_none_or(|(distance, _)| leaf_distance < distance)
                {
                    canopy = Some((leaf_distance, tree.kind));
                }
            }
            canopy.map_or(state, |(distance, kind)| {
                self.palette.leaves[kind][distance - 1]
            })
        })
    }

    /// Materializes only one 16³ section. Section coordinates are signed world Y.
    /// Cells outside a partial physical-domain footprint are non-queryable air
    /// padding, not procedural samples or playable source coverage.
    pub fn materialize_section(&self, section_y: i32) -> Result<ChunkSection, PrepareError> {
        let bottom = section_y
            .checked_mul(16)
            .ok_or(PrepareError::Invalid("base section coordinate overflow"))?;
        if !(MIN_Y..MAX_Y).contains(&bottom) {
            return Err(PrepareError::Invalid("base section outside compact height"));
        }
        let mut section = ChunkSection::filled(self.palette.air, self.palette.air);
        for y in 0..16_u8 {
            for z in 0..16_u8 {
                for x in 0..16_u8 {
                    let Some(state) = self.sample(x, bottom + i32::from(y), z) else {
                        continue;
                    };
                    if state != self.palette.air {
                        section.set(x, y, z, state);
                    }
                }
            }
        }
        Ok(section)
    }

    /// No gameplay activation or durable record; derived lighting is a later stage.
    pub fn materialize_chunk(&self) -> Result<Chunk, PrepareError> {
        if let ReadyBase::Basin(chunk) = &self.base {
            return Ok((**chunk).clone());
        }
        let mut chunk = Chunk::empty(
            self.position,
            self.palette.air,
            Identifier::parse("minecraft:plains").expect("static biome identifier"),
        );
        for (index, section) in chunk.sections.iter_mut().enumerate() {
            *section = self.materialize_section(MIN_Y / 16 + index as i32)?;
        }
        chunk.status = "minecraft:full".into();
        chunk.dirty = false;
        Ok(chunk)
    }

    /// Transfers an already materialized W0 chunk without copying its storage.
    pub fn into_chunk(self) -> Result<Chunk, PrepareError> {
        match self {
            Self {
                base: ReadyBase::Basin(chunk),
                ..
            } => Ok(*chunk),
            ready => ready.materialize_chunk(),
        }
    }
}

pub struct BaseMaterializer<'a> {
    definition: &'a WorldDefinition,
    identity: WorldIdentity,
    root: &'a WorldPlan,
    palette: Palette,
    seed: u64,
}
impl<'a> BaseMaterializer<'a> {
    pub fn new(
        definition: &'a WorldDefinition,
        identity: WorldIdentity,
        root: &'a WorldPlan,
        blocks: &BlockRegistry,
    ) -> Result<Self, PrepareError> {
        Ok(Self::with_palette(
            definition,
            identity,
            root,
            Palette::new(blocks)?,
        ))
    }

    pub(crate) fn with_palette(
        definition: &'a WorldDefinition,
        identity: WorldIdentity,
        root: &'a WorldPlan,
        palette: Palette,
    ) -> Self {
        Self {
            definition,
            identity,
            root,
            palette,
            seed: u64::from_le_bytes(identity.bytes()[..8].try_into().expect("identity prefix")),
        }
    }

    pub fn prepare_chunk(
        &self,
        position: ChunkPos,
        blocks: &BlockRegistry,
    ) -> Result<PreparedBaseChunk, PrepareError> {
        self.prepare_chunk_with(position, blocks, |planner, node| {
            planner.prepare_cold(node).map(|(plan, _)| Arc::new(plan))
        })
    }

    /// Optional discardable-cache resolver. It runs only during preparation;
    /// imported coverage should be resolved by NativeStore before calling this.
    /// The returned plan must match this identity and the requested leaf address.
    pub fn prepare_chunk_with(
        &self,
        position: ChunkPos,
        blocks: &BlockRegistry,
        mut region: impl FnMut(&RegionPlanner<'_>, GeoNodeId) -> Result<Arc<RegionPlan>, PrepareError>,
    ) -> Result<PreparedBaseChunk, PrepareError> {
        let x = i64::from(position.x) * 16;
        let z = i64::from(position.z) * 16;
        let domain = self.definition.input().domain;
        if x + 16 <= domain.min_x
            || z + 16 <= domain.min_z
            || x >= domain.max_x_exclusive
            || z >= domain.max_z_exclusive
        {
            return Err(PrepareError::Invalid("base chunk outside physical domain"));
        }
        let base = match self.root {
            WorldPlan::BasinFixture(basin) => {
                ReadyBase::Basin(Box::new(basin.materialize_chunk(position, blocks).ok_or(
                    PrepareError::Invalid("cannot materialize checked W0 basin chunk"),
                )?))
            }
            WorldPlan::Plane(root) => {
                let planner = RegionPlanner::new(root, self.identity);
                let scale = self.definition.input().horizontal_meters_per_block;
                let mut ready: Option<Arc<RegionPlan>> = None;
                let mut columns = Vec::with_capacity(256);
                for local_z in 0..16 {
                    for local_x in 0..16 {
                        let world_x = x + local_x;
                        let world_z = z + local_z;
                        if world_x < domain.min_x
                            || world_x >= domain.max_x_exclusive
                            || world_z < domain.min_z
                            || world_z >= domain.max_z_exclusive
                        {
                            columns.push(None);
                            continue;
                        }
                        let x_m = world_x as f64 * scale;
                        let z_m = world_z as f64 * scale;
                        let node = planner.leaf_at(x_m, z_m)?;
                        if ready.as_ref().is_none_or(|plan| plan.node != node) {
                            let plan = region(&planner, node)?;
                            if plan.identity != self.identity
                                || plan.node != node
                                || plan.version != super::region::REGION_VERSION
                            {
                                return Err(PrepareError::Invalid("foreign prepared base region"));
                            }
                            ready = Some(plan);
                        }
                        let column = ready.as_ref().expect("prepared leaf").sample(x_m, z_m)?;
                        columns.push(Some(RenderedColumn::from_plan(
                            &column,
                            self.palette,
                            world_x,
                            world_z,
                            self.seed,
                        )));
                    }
                }
                ReadyBase::Plane(
                    columns
                        .into_boxed_slice()
                        .try_into()
                        .map_err(|_| PrepareError::Invalid("base column count mismatch"))?,
                )
            }
        };
        let trees = self.prepare_trees(position, &mut region)?;
        Ok(PreparedBaseChunk {
            position,
            identity: self.identity,
            palette: self.palette,
            base,
            trees,
        })
    }

    pub fn natural_column(&self, x: i32, z: i32) -> Result<NaturalColumnSample, PrepareError> {
        self.natural_column_with(i64::from(x), i64::from(z), &mut |planner, node| {
            planner.prepare_cold(node).map(|(plan, _)| Arc::new(plan))
        })
    }

    pub fn natural_column_with(
        &self,
        x: i64,
        z: i64,
        region: &mut impl FnMut(&RegionPlanner<'_>, GeoNodeId) -> Result<Arc<RegionPlan>, PrepareError>,
    ) -> Result<NaturalColumnSample, PrepareError> {
        let domain = self.definition.input().domain;
        if x < domain.min_x
            || x >= domain.max_x_exclusive
            || z < domain.min_z
            || z >= domain.max_z_exclusive
        {
            return Err(PrepareError::Invalid(
                "natural column outside physical domain",
            ));
        }
        let root = self.root.plane().ok_or(PrepareError::Invalid(
            "live natural facts require a physical plane root",
        ))?;
        let planner = RegionPlanner::new(root, self.identity);
        let scale = self.definition.input().horizontal_meters_per_block;
        let (x_m, z_m) = (x as f64 * scale, z as f64 * scale);
        let node = planner.leaf_at(x_m, z_m)?;
        let plan = region(&planner, node)?;
        if plan.identity != self.identity
            || plan.node != node
            || plan.version != super::region::REGION_VERSION
        {
            return Err(PrepareError::Invalid("foreign natural region"));
        }
        plan.sample(x_m, z_m)
    }

    fn prepare_trees(
        &self,
        position: ChunkPos,
        region: &mut impl FnMut(&RegionPlanner<'_>, GeoNodeId) -> Result<Arc<RegionPlan>, PrepareError>,
    ) -> Result<Vec<Tree>, PrepareError> {
        if self.root.plane().is_none() {
            return Ok(Vec::new());
        }
        let domain = self.definition.input().domain;
        let (min_x, min_z) = (i64::from(position.x) * 16, i64::from(position.z) * 16);
        let mut trees = Vec::with_capacity(16);
        // Stable eight-block anchors and a two-block canopy halo.
        for cell_z in (min_z - 2).div_euclid(8)..=(min_z + 17).div_euclid(8) {
            for cell_x in (min_x - 2).div_euclid(8)..=(min_x + 17).div_euclid(8) {
                let hash = feature_hash(self.seed, cell_x, 0, cell_z);
                let (x, z) = (
                    cell_x * 8 + (hash & 7) as i64,
                    cell_z * 8 + ((hash >> 3) & 7) as i64,
                );
                if x < domain.min_x
                    || x >= domain.max_x_exclusive
                    || z < domain.min_z
                    || z >= domain.max_z_exclusive
                {
                    continue;
                }
                let column = self.natural_column_with(x, z, region)?;
                let ecology = &column.geology_ecology.ecology;
                let moisture = ecology.moisture_fraction.iter().sum::<f64>() * 0.25;
                let temperature = ecology.temperature_c.iter().sum::<f64>() * 0.25;
                let density = (moisture * (1.0 - ecology.rock_exposure) * 0.8).clamp(0.0, 0.65);
                if column.water_surface_y.is_some()
                    || ecology.wet
                    || ecology.soil_depth_m < 0.25
                    || ecology.slope > 0.7
                    || temperature < -8.0
                    || (hash % 1000) as f64 >= density * 1000.0
                {
                    continue;
                }
                trees.push(Tree {
                    x,
                    z,
                    ground_y: column.ground_y,
                    height: 4 + ((hash >> 8) % 3) as i32,
                    kind: usize::from(temperature < 8.0),
                });
            }
        }
        Ok(trees)
    }
}

struct Tree {
    x: i64,
    z: i64,
    ground_y: i32,
    height: i32,
    kind: usize,
}

fn feature_hash(seed: u64, x: i64, y: i64, z: i64) -> u64 {
    let mut h = seed
        ^ (x as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ (z as u64).wrapping_mul(0x94D0_49BB_1331_11EB);
    h = (h ^ (h >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h = (h ^ (h >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^ (h >> 31)
}

fn surface_state(column: &NaturalColumnSample, palette: Palette) -> BlockStateId {
    let ecology = &column.geology_ecology.ecology;
    if column.water_surface_y.is_some() {
        return palette.rock(ecology.parent_material);
    }
    let temperature = ecology.temperature_c.iter().sum::<f64>() * 0.25;
    if ecology.snow_fraction.iter().sum::<f64>() > 2.0 && temperature < 0.0 {
        palette.snow
    } else if ecology.rock_exposure > 0.65 || ecology.soil_depth_m < 0.25 {
        palette.rock(ecology.parent_material)
    } else if ecology.water_deficit_mm.iter().sum::<f64>() > 900.0 {
        palette.sand
    } else if temperature < 8.0 && ecology.moisture_fraction.iter().sum::<f64>() > 1.6 {
        palette.podzol
    } else {
        palette.grass
    }
}

#[cfg(test)]
#[path = "materialization_tests.rs"]
mod tests;
