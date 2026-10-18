//! Native compact Earth source. All preparation runs on the caller's load worker.
//! Natural queries and village plans never activate entities or write native state.
use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use mc_data::Identifier;
use mc_data::items::ItemRegistry;
use mc_data::loot::chest_26_1_2::ChestLootCatalog;
use mc_world::native::NativeError;
use mc_world::storage::{NativeNaturalSource, NaturalTerrainColumn};
use mc_world::{
    BiomeSection, BlockRegistry, BlockStateId, Chunk, ChunkPos, GeneratedVillageSite, Heightmap,
    MAX_Y, MIN_Y, PackedBitArray,
};

use super::definition::{PlayableBounds, WorldDefinition, WorldIdentity};
use super::materialization::{BaseMaterializer, Palette};
use super::prepared::{PrepareError, PreparedWorld};
use super::region::{GeoNodeId, NaturalColumnSample, RegionPlan, RegionPlanner};
use super::region_water::WaterPlan;
use super::world_plan::WorldPlan;
use crate::structures::StructureLoot;
use crate::terrain::{SpawnLocation, TerrainDiagnosticSample};
use crate::village::plan_source::{VillagePlan, VillagePlanSet, VillagePlanSource};

const REGION_CACHE_ENTRIES: usize = 16;
const VILLAGE_CACHE_ENTRIES: usize = 64;
const MAX_VILLAGE_FOOTPRINT_COLUMNS: i64 = 512 * 512;
const MAX_SITE_PAGE_CHUNKS: i64 = 4096;
const MAX_SPAWN_ROOT_CANDIDATES: usize = 4096;
// Physical highland/relief gates, independent of atlas spacing and compact block Y.
const ROCKY_PEAK_MIN_ALTITUDE_M: f64 = 1_500.0;
const ROCKY_PEAK_MIN_SLOPE: f64 = 0.1;

pub struct EarthRuntime {
    world: Arc<PreparedWorld>,
    blocks: Arc<BlockRegistry>,
    villages: Option<Arc<VillagePlanSource>>,
    loot: ChestLootCatalog,
    items: Arc<ItemRegistry>,
    air: BlockStateId,
    palette: Palette,
    cache: EarthSourceCache,
}

type RegionCache = BTreeMap<([u8; 32], GeoNodeId), Arc<RegionPlan>>;
type VillageKey = ([u8; 32], (i32, i32));

/// Only completed facts are shared. Cold footprint scans run on the caller's
/// worker, outside every cache lock; concurrent misses may compute equal facts.
#[derive(Default)]
struct VillageSuitabilityCache {
    entries: HashMap<VillageKey, bool>,
    order: VecDeque<VillageKey>,
}

impl VillageSuitabilityCache {
    fn insert(&mut self, key: VillageKey, dry: bool) -> bool {
        if let Some(existing) = self.entries.get(&key) {
            return *existing;
        }
        if self.entries.len() == VILLAGE_CACHE_ENTRIES
            && let Some(evicted) = self.order.pop_front()
        {
            self.entries.remove(&evicted);
        }
        self.entries.insert(key, dry);
        self.order.push_back(key);
        dry
    }
}

/// Discardable bounded cache; never persistent world state.
#[derive(Default)]
pub struct EarthSourceCache {
    regions: Mutex<RegionCache>,
    dry_villages: Mutex<VillageSuitabilityCache>,
}

/// Borrowed immutable base source shared by live loading and offline inspection.
pub struct EarthBaseSource<'a> {
    definition: &'a WorldDefinition,
    identity: WorldIdentity,
    root: &'a WorldPlan,
    blocks: &'a BlockRegistry,
    villages: Option<&'a VillagePlanSource>,
    loot: &'a ChestLootCatalog,
    items: &'a ItemRegistry,
    air: BlockStateId,
    palette: Palette,
    cache: &'a EarthSourceCache,
}

impl<'a> EarthBaseSource<'a> {
    pub fn new(
        definition: &'a WorldDefinition,
        identity: WorldIdentity,
        root: &'a WorldPlan,
        blocks: &'a BlockRegistry,
        villages: Option<&'a VillagePlanSource>,
        loot: StructureLoot<'a>,
        cache: &'a EarthSourceCache,
    ) -> Result<Self, PrepareError> {
        if root.plane().is_none() {
            return Err(PrepareError::Invalid(
                "Earth runtime base requires a prepared physical plane root",
            ));
        }
        if loot.seed != definition.input().seed {
            return Err(PrepareError::Invalid(
                "Earth loot seed differs from immutable definition",
            ));
        }
        let palette = Palette::new(blocks)?;
        let air = blocks
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .ok_or(PrepareError::Invalid("Earth air state missing"))?
            .default;
        Ok(Self {
            definition,
            identity,
            root,
            blocks,
            villages,
            loot: loot.catalog,
            items: loot.items,
            cache,
            air,
            palette,
        })
    }

    pub fn identity(&self) -> WorldIdentity {
        self.identity
    }

    fn materializer(&self) -> BaseMaterializer<'_> {
        BaseMaterializer::with_palette(self.definition, self.identity, self.root, self.palette)
    }

    fn region(
        &self,
        planner: &RegionPlanner<'_>,
        node: GeoNodeId,
    ) -> Result<Arc<RegionPlan>, PrepareError> {
        if let Some(plan) = self
            .cache
            .regions
            .lock()
            .map_err(|_| PrepareError::Invalid("Earth region cache poisoned"))?
            .get(&(self.identity.bytes(), node))
        {
            return Ok(Arc::clone(plan));
        }
        let plan = Arc::new(planner.prepare(node)?);
        let mut cache = self
            .cache
            .regions
            .lock()
            .map_err(|_| PrepareError::Invalid("Earth region cache poisoned"))?;
        if let Some(existing) = cache.get(&(self.identity.bytes(), node)) {
            return Ok(Arc::clone(existing));
        }
        let evicted = if cache.len() == REGION_CACHE_ENTRIES {
            cache.pop_first()
        } else {
            None
        };
        cache.insert((self.identity.bytes(), node), Arc::clone(&plan));
        // A last-reference region drop can release large prepared arrays.
        drop(cache);
        drop(evicted);
        Ok(plan)
    }

    pub fn natural_column(&self, x: i32, z: i32) -> Result<NaturalColumnSample, PrepareError> {
        self.materializer()
            .natural_column_with(i64::from(x), i64::from(z), &mut |planner, node| {
                self.region(planner, node)
            })
    }

    pub fn free_surface_height(&self, x: i32, z: i32) -> Result<i32, PrepareError> {
        Ok(self.natural_column(x, z)?.ground_y)
    }
    pub fn biome_at(&self, x: i32, z: i32) -> Result<Identifier, PrepareError> {
        Ok(natural_biome(&self.natural_column(x, z)?))
    }

    pub fn diagnostic_sample(
        &self,
        x: i32,
        z: i32,
    ) -> Result<TerrainDiagnosticSample, PrepareError> {
        let column = self.natural_column(x, z)?;
        let ecology = &column.geology_ecology.ecology;
        Ok(TerrainDiagnosticSample {
            surface_y: column.ground_y,
            biome: natural_biome(&column),
            vegetation_density: column.water_surface_y.is_none().then(|| {
                ecology.moisture_fraction.iter().sum::<f64>() * 0.25 * (1.0 - ecology.rock_exposure)
            }),
        })
    }

    // The existing village solver takes infallible callbacks. Bridge errors once,
    // rejecting the query after the solver rather than concealing a failed fact.
    fn with_village_facts<T>(
        &self,
        query: impl FnOnce(&dyn Fn(i32, i32) -> i32, &dyn Fn(i32, i32, i32) -> Identifier) -> T,
    ) -> Result<T, PrepareError> {
        let error = RefCell::new(None);
        let height = |x, z| {
            if !self.contains_column(x, z) {
                return MIN_Y + 1;
            }
            match self.free_surface_height(x, z) {
                Ok(y) => y.saturating_add(1),
                Err(cause) => {
                    *error.borrow_mut() = Some(cause);
                    MIN_Y + 1
                }
            }
        };
        let biome = |x, _y, z| {
            if !self.contains_column(x, z) {
                return Identifier::parse("minecraft:ocean").expect("static identifier");
            }
            match self.biome_at(x, z) {
                Ok(biome) => biome,
                Err(cause) => {
                    *error.borrow_mut() = Some(cause);
                    Identifier::parse("minecraft:ocean").expect("static identifier")
                }
            }
        };
        let result = query(&height, &biome);
        if let Some(cause) = error.into_inner() {
            return Err(cause);
        }
        Ok(result)
    }

    fn dry_village(&self, plan: &VillagePlan) -> Result<bool, PrepareError> {
        let key = (self.identity.bytes(), plan.start_chunk());
        if let Some(dry) = self
            .cache
            .dry_villages
            .lock()
            .map_err(|_| PrepareError::Invalid("Earth village cache poisoned"))?
            .entries
            .get(&key)
            .copied()
        {
            return Ok(dry);
        }
        // A failed scan is never cached, so a later caller retries the real facts.
        let dry = self.compute_dry_village(plan)?;
        let mut cache = self
            .cache
            .dry_villages
            .lock()
            .map_err(|_| PrepareError::Invalid("Earth village cache poisoned"))?;
        Ok(cache.insert(key, dry))
    }

    fn compute_dry_village(&self, plan: &VillagePlan) -> Result<bool, PrepareError> {
        let (min, max) = plan.affected_box();
        let width = i64::from(max.x) - i64::from(min.x) + 1;
        let depth = i64::from(max.z) - i64::from(min.z) + 1;
        if width <= 0
            || depth <= 0
            || width
                .checked_mul(depth)
                .is_none_or(|area| area > MAX_VILLAGE_FOOTPRINT_COLUMNS)
        {
            return Err(PrepareError::Invalid(
                "Earth village footprint exceeds preparation bound",
            ));
        }
        let domain = self.definition.input().domain;
        let mut dry = i64::from(min.x) >= domain.min_x
            && i64::from(max.x) < domain.max_x_exclusive
            && i64::from(min.z) >= domain.min_z
            && i64::from(max.z) < domain.max_z_exclusive;
        if dry {
            'columns: for z in min.z..=max.z {
                for x in min.x..=max.x {
                    let column = self.natural_column(x, z)?;
                    if column.water_surface_y.is_some()
                        || column.protected
                        || column.geology_ecology.ecology.wet
                        || column.geology_ecology.ecology.slope > 0.7
                    {
                        dry = false;
                        break 'columns;
                    }
                }
            }
        }
        Ok(dry)
    }

    fn village_plans(&self, position: ChunkPos) -> Result<Option<VillagePlanSet>, PrepareError> {
        let Some(source) = self.villages.as_ref() else {
            return Ok(None);
        };
        let plans = self.with_village_facts(|height, biome| {
            source.plans_for_chunk(position.x, position.z, height, biome)
        })?;
        let Some(plans) = plans else {
            return Ok(None);
        };
        let mut accepted = Vec::new();
        for plan in plans.plans() {
            if self.dry_village(plan)? {
                accepted.push(Arc::clone(plan));
            }
        }
        Ok(VillagePlanSet::from_plans(accepted))
    }

    pub fn village_sites_in_region(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Result<Vec<GeneratedVillageSite>, PrepareError> {
        let width = i64::from(max.0) - i64::from(min.0) + 1;
        let depth = i64::from(max.1) - i64::from(min.1) + 1;
        if width <= 0
            || depth <= 0
            || width
                .checked_mul(depth)
                .is_none_or(|area| area > MAX_SITE_PAGE_CHUNKS)
        {
            return Err(PrepareError::Invalid(
                "Earth village site page exceeds bound",
            ));
        }
        let Some(source) = self.villages.as_ref() else {
            return Ok(Vec::new());
        };
        let mut sites = Vec::new();
        for z in min.1..=max.1 {
            for x in min.0..=max.0 {
                if let Some(plan) = self.with_village_facts(|height, biome| {
                    source.plan_for_start_chunk(x, z, height, biome)
                })? && self.dry_village(&plan)?
                {
                    sites.push(plan.site());
                }
            }
        }
        Ok(sites)
    }

    pub fn locate_safe_spawn(
        &self,
        playable: PlayableBounds,
    ) -> Result<SpawnLocation, PrepareError> {
        playable.validate(self.definition)?;
        let root = self.root.plane().expect("constructor checks plane");
        let bounds = playable.bounds;
        if bounds.max_x_exclusive - bounds.min_x <= 16
            || bounds.max_z_exclusive - bounds.min_z <= 16
        {
            return Err(PrepareError::Invalid(
                "playable bounds too small for safe land spawn clearance",
            ));
        }
        let scale = self.definition.input().horizontal_meters_per_block;
        let min = root
            .grid
            .index_at(bounds.min_x as f64 * scale, bounds.min_z as f64 * scale);
        let max = root.grid.index_at(
            (bounds.max_x_exclusive - 1) as f64 * scale,
            (bounds.max_z_exclusive - 1) as f64 * scale,
        );
        let (min_x, min_z) = (min % root.grid.columns, min / root.grid.columns);
        let (max_x, max_z) = (max % root.grid.columns, max / root.grid.columns);
        let total = (max_x - min_x + 1)
            .checked_mul(max_z - min_z + 1)
            .ok_or(PrepareError::Invalid("spawn index extent overflow"))?;
        let stride = total.div_ceil(MAX_SPAWN_ROOT_CANDIDATES).max(1);
        for ordinal in (0..total).step_by(stride).take(MAX_SPAWN_ROOT_CANDIDATES) {
            let index = (min_z + ordinal / (max_x - min_x + 1)) * root.grid.columns
                + min_x
                + ordinal % (max_x - min_x + 1);
            if root.ocean[index] || root.elevation_m[index] <= 0.0 {
                continue;
            }
            let (x_m, z_m) = root.grid.center(index);
            let x =
                ((x_m / scale).round() as i64).clamp(bounds.min_x + 8, bounds.max_x_exclusive - 9);
            let z =
                ((z_m / scale).round() as i64).clamp(bounds.min_z + 8, bounds.max_z_exclusive - 9);
            let (Ok(x), Ok(z)) = (i32::try_from(x), i32::try_from(z)) else {
                continue;
            };
            let center = self.natural_column(x, z)?;
            if center.water_surface_y.is_some()
                || center.protected
                || center.geology_ecology.ecology.wet
            {
                continue;
            }
            let mut relief_ok = true;
            for dz in [-8, 0, 8] {
                for dx in [-8, 0, 8] {
                    let column = self.natural_column(x + dx, z + dz)?;
                    relief_ok &= column.water_surface_y.is_none()
                        && !column.protected
                        && (column.ground_y - center.ground_y).abs() <= 3;
                }
            }
            if !relief_ok {
                continue;
            }
            let position = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            let base = self.materialize_chunk(position)?;
            let (lx, lz) = (x.rem_euclid(16) as u8, z.rem_euclid(16) as u8);
            if base
                .get_block(lx, center.ground_y, lz)
                .is_none_or(|block| block == self.air)
                || base.get_block(lx, center.ground_y + 1, lz) != Some(self.air)
                || base.get_block(lx, center.ground_y + 2, lz) != Some(self.air)
            {
                continue;
            }
            return Ok(SpawnLocation {
                block_x: x,
                surface_y: center.ground_y,
                block_z: z,
            });
        }
        Err(PrepareError::Invalid(
            "no safe dry land spawn in bounded indexed root candidates",
        ))
    }

    pub fn materialize_chunk(&self, position: ChunkPos) -> Result<Chunk, PrepareError> {
        let materializer = self.materializer();
        let mut chunk = materializer
            .prepare_chunk_with(position, self.blocks, |planner, node| {
                self.region(planner, node)
            })?
            .into_chunk()?;
        if let Some(plans) = self.village_plans(position)? {
            for z in 0..16u8 {
                for x in 0..16u8 {
                    let (wx, wz) = (
                        position.x * 16 + i32::from(x),
                        position.z * 16 + i32::from(z),
                    );
                    if !plans.affects_column(wx, wz) {
                        continue;
                    }
                    let ground = self.free_surface_height(wx, wz)?;
                    let adjusted = plans.adjusted_surface_y(wx, wz, ground, MIN_Y, MAX_Y);
                    let surface = chunk
                        .get_block(x, ground, z)
                        .ok_or(PrepareError::Invalid("missing Earth village base column"))?;
                    if adjusted > ground {
                        for y in ground + 1..=adjusted {
                            chunk.set_block(x, y, z, surface);
                        }
                    }
                    if adjusted < ground {
                        for y in adjusted + 1..=ground + 8 {
                            chunk.set_block(x, y, z, self.air);
                        }
                        chunk.set_block(x, adjusted, z, surface);
                    }
                }
            }
            let source = self.villages.as_ref().expect("plan source exists");
            let error = RefCell::new(None);
            let surface = |x, z| match self.free_surface_height(x, z) {
                Ok(y) => y,
                Err(cause) => {
                    *error.borrow_mut() = Some(cause);
                    MIN_Y
                }
            };
            let loot = StructureLoot {
                seed: self.definition.input().seed,
                catalog: self.loot,
                items: self.items,
            };
            source
                .write_plans(
                    &mut chunk,
                    &plans,
                    &mut [false; 256],
                    &surface,
                    self.air,
                    &loot,
                )
                .map_err(|cause| PrepareError::Committed {
                    identity: self.identity().hex(),
                    reason: cause.to_string(),
                })?;
            if let Some(cause) = error.into_inner() {
                return Err(cause);
            }
            let mut markers = chunk.generated_entities();
            for marker in &mut markers {
                marker.claim = format!("native:{}:{}", self.identity().hex(), marker.claim);
            }
            if !markers.is_empty() {
                chunk.set_generated_entities(&markers);
            }
        }
        self.derive_chunk(&mut chunk)?;
        chunk.dirty = false;
        Ok(chunk)
    }

    fn derive_chunk(&self, chunk: &mut Chunk) -> Result<(), PrepareError> {
        let mut world_surface = Heightmap::zeroed();
        for z in 0..16u8 {
            for x in 0..16u8 {
                let top = (MIN_Y..MAX_Y).rev().find(|y| {
                    chunk
                        .get_block(x, *y, z)
                        .is_some_and(|state| state != self.air)
                });
                world_surface.set(x, z, top.map_or(0, |y| (y + 1 - MIN_Y) as u32));
            }
        }
        chunk
            .heightmaps
            .insert("WORLD_SURFACE".into(), world_surface.clone());
        chunk
            .heightmaps
            .insert("MOTION_BLOCKING".into(), world_surface);
        // Each 4x4 horizontal biome cell reads the same prepared natural facts.
        let mut palette = Vec::new();
        let mut indices = PackedBitArray::zeroed(6, 64);
        for z in 0..4 {
            for x in 0..4 {
                let wx = i64::from(chunk.pos.x) * 16 + x as i64 * 4 + 2;
                let wz = i64::from(chunk.pos.z) * 16 + z as i64 * 4 + 2;
                let domain = self.definition.input().domain;
                let biome = if wx < domain.min_x
                    || wx >= domain.max_x_exclusive
                    || wz < domain.min_z
                    || wz >= domain.max_z_exclusive
                {
                    Identifier::parse("minecraft:plains").expect("padding biome")
                } else {
                    self.biome_at(wx as i32, wz as i32)?
                };
                let index = palette
                    .iter()
                    .position(|entry| entry == &biome)
                    .unwrap_or_else(|| {
                        palette.push(biome);
                        palette.len() - 1
                    });
                for y in 0..4 {
                    indices.set(y * 16 + z * 4 + x, index as u32);
                }
            }
        }
        let section = BiomeSection::from_indirect(palette, indices);
        for biomes in &mut chunk.biomes {
            *biomes = section.clone();
        }
        Ok(())
    }
}

impl EarthRuntime {
    pub fn new(
        world: Arc<PreparedWorld>,
        blocks: Arc<BlockRegistry>,
        villages: Option<Arc<VillagePlanSource>>,
    ) -> Result<Self, PrepareError> {
        if world.root_plan().plane().is_none() {
            return Err(PrepareError::Invalid(
                "live Earth runtime requires a prepared physical plane root",
            ));
        }
        let palette = Palette::new(&blocks)?;
        let air = blocks
            .block(&Identifier::parse("minecraft:air").expect("static identifier"))
            .ok_or(PrepareError::Invalid("Earth air state missing"))?
            .default;
        Ok(Self {
            world,
            blocks,
            villages,
            loot: ChestLootCatalog::new(),
            items: Arc::new(ItemRegistry::default()),
            air,
            palette,
            cache: EarthSourceCache::default(),
        })
    }

    /// Retain the world's item decoder even when optional chest tables are absent.
    /// Callers must supply an empty catalog plus the actual registry on fallback.
    pub fn with_chest_loot(mut self, loot: ChestLootCatalog, items: Arc<ItemRegistry>) -> Self {
        self.loot = loot;
        self.items = items;
        self
    }

    fn base_source(&self) -> EarthBaseSource<'_> {
        EarthBaseSource {
            definition: self.world.definition(),
            identity: self.world.identity(),
            root: self.world.root_plan(),
            blocks: &self.blocks,
            villages: self.villages.as_deref(),
            loot: &self.loot,
            items: &self.items,
            air: self.air,
            palette: self.palette,
            cache: &self.cache,
        }
    }

    pub fn identity(&self) -> WorldIdentity {
        self.world.identity()
    }
    pub fn village_plan_source(&self) -> Option<&Arc<VillagePlanSource>> {
        self.villages.as_ref()
    }
    pub fn natural_column(&self, x: i32, z: i32) -> Result<NaturalColumnSample, PrepareError> {
        self.base_source().natural_column(x, z)
    }
    pub fn free_surface_height(&self, x: i32, z: i32) -> Result<i32, PrepareError> {
        self.base_source().free_surface_height(x, z)
    }
    pub fn biome_at(&self, x: i32, z: i32) -> Result<Identifier, PrepareError> {
        self.base_source().biome_at(x, z)
    }
    pub fn diagnostic_sample(
        &self,
        x: i32,
        z: i32,
    ) -> Result<TerrainDiagnosticSample, PrepareError> {
        self.base_source().diagnostic_sample(x, z)
    }
    pub fn village_sites_in_region(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Result<Vec<GeneratedVillageSite>, PrepareError> {
        self.base_source().village_sites_in_region(min, max)
    }
    pub fn locate_safe_spawn(&self) -> Result<SpawnLocation, PrepareError> {
        self.base_source().locate_safe_spawn(self.world.playable())
    }
}

impl NativeNaturalSource for EarthBaseSource<'_> {
    fn seed(&self) -> i64 {
        self.definition.input().seed
    }

    fn contains_column(&self, x: i32, z: i32) -> bool {
        let domain = self.definition.input().domain;
        i64::from(x) >= domain.min_x
            && i64::from(x) < domain.max_x_exclusive
            && i64::from(z) >= domain.min_z
            && i64::from(z) < domain.max_z_exclusive
    }

    fn prepare_columns(
        &self,
        columns: &[(i32, i32)],
    ) -> Result<Vec<NaturalTerrainColumn>, NativeError> {
        columns
            .iter()
            .map(|&(x, z)| {
                let column = self
                    .natural_column(x, z)
                    .map_err(|error| NativeError::Source(error.to_string()))?;
                Ok(NaturalTerrainColumn {
                    position: (x, z),
                    ground_y: column.ground_y,
                    biome: natural_biome(&column),
                    water_surface_y: column.water_surface_y,
                    protected: column.protected,
                    stable_ground: !column.geology_ecology.ecology.wet
                        && column.geology_ecology.ecology.slope <= 0.7,
                })
            })
            .collect()
    }

    fn prepare_village_sites(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Result<Vec<GeneratedVillageSite>, NativeError> {
        self.village_sites_in_region(min, max)
            .map_err(|error| NativeError::Source(error.to_string()))
    }
}

impl NativeNaturalSource for EarthRuntime {
    fn seed(&self) -> i64 {
        self.base_source().seed()
    }

    fn contains_column(&self, x: i32, z: i32) -> bool {
        self.base_source().contains_column(x, z)
    }

    fn prepare_columns(
        &self,
        columns: &[(i32, i32)],
    ) -> Result<Vec<NaturalTerrainColumn>, NativeError> {
        self.base_source().prepare_columns(columns)
    }

    fn prepare_village_sites(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Result<Vec<GeneratedVillageSite>, NativeError> {
        self.base_source().prepare_village_sites(min, max)
    }
}

pub fn natural_biome(column: &NaturalColumnSample) -> Identifier {
    let ecology = &column.geology_ecology.ecology;
    let temperature = ecology.temperature_c.iter().sum::<f64>() * 0.25;
    let moisture = ecology.moisture_fraction.iter().sum::<f64>() * 0.25;
    let name = match &column.water {
        WaterPlan::Ocean { .. } => "minecraft:ocean",
        WaterPlan::River { .. } => {
            if temperature < 0.0 {
                "minecraft:frozen_river"
            } else {
                "minecraft:river"
            }
        }
        WaterPlan::Lake { .. } => "minecraft:river",
        // Exposure describes thin soil/substrate, not mountain relief. Climate
        // already includes the physical lapse correction in region_geology.
        WaterPlan::None
            if ecology.rock_exposure > 0.65
                && column.ground_m >= ROCKY_PEAK_MIN_ALTITUDE_M
                && ecology.slope >= ROCKY_PEAK_MIN_SLOPE =>
        {
            "minecraft:stony_peaks"
        }
        WaterPlan::None if temperature < 0.0 => "minecraft:snowy_plains",
        WaterPlan::None if ecology.water_deficit_mm.iter().sum::<f64>() > 900.0 => {
            "minecraft:desert"
        }
        WaterPlan::None if temperature < 8.0 && moisture > 0.4 => "minecraft:taiga",
        WaterPlan::None if temperature > 23.0 && moisture > 0.65 => "minecraft:jungle",
        WaterPlan::None if moisture > 0.5 => "minecraft:forest",
        WaterPlan::None => "minecraft:plains",
    };
    Identifier::parse(name).expect("static Earth biome")
}

#[cfg(test)]
#[path = "runtime_tests.rs"]
mod tests;
