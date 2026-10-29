//! Native water/lava scheduled work and atomic source/bucket interactions.
//!
//! Absolute deadlines, fluid identity, priority and insertion order survive
//! WAL/checkpoint recovery; unloaded work pauses without rebasing. Still
//! fluid is scheduled only by neighbor writes or OLD lava ignition fuel.
//! Waterlogged sources retain their enclosing block; buckets fill/drain them
//! explicitly, while flowing water never washes a waterloggable block away.
//! Flow uses bounded shortest-descent selection and collision-face occlusion.
//! Water/lava delays and horizontal drop-off follow the owning dimension.

use crate::boundary::{BlockPos, EntityId};
use crate::registry::ResolvedStates;
use crate::tick::TickState;
use mc_data::{Identifier, block_facts::FluidKind};
use std::sync::LazyLock;

#[path = "fluid_ticks.rs"]
mod ticks;
pub use ticks::{FluidTickImage, FluidTicks, FluidTicksImage};

/// Waterlogged blocks contain a source without becoming liquid blocks.
pub(crate) fn contained(state: &TickState, block: u32) -> Option<(Fluid, u8)> {
    fluid_at(&state.registry.states, block).or_else(|| {
        state
            .registry
            .blocks
            .by_id(mc_world::BlockStateId(block))
            .filter(|entry| {
                entry.block.id.path() == "bubble_column"
                    || entry
                        .properties
                        .iter()
                        .any(|(key, value)| key == "waterlogged" && value == "true")
            })
            .map(|_| (Fluid::Water, 0))
    })
}

/// Point immersion includes flowing fluid height and pose-dependent eye samples.
pub(crate) fn water_at_point(state: &TickState, point: [f64; 3]) -> bool {
    let [x, y, z] = point.map(|v| v.floor() as i32);
    let Some((Fluid::Water, level)) = state
        .world
        .block_at(x, y, z)
        .and_then(|id| contained(state, id))
    else {
        return false;
    };
    let above = state
        .world
        .block_at(x, y + 1, z)
        .and_then(|id| contained(state, id))
        .is_some_and(|(fluid, _)| fluid == Fluid::Water);
    let height = if above {
        1.0
    } else {
        f64::from(amount(level)) / 9.0
    };
    point[1] < f64::from(y) + height
}

fn passage(state: &TickState, from: BlockPos, to: BlockPos) -> bool {
    let (axis, positive) = if from.x != to.x {
        (0, to.x > from.x)
    } else if from.y != to.y {
        (1, to.y > from.y)
    } else {
        (2, to.z > from.z)
    };
    let face = |pos: BlockPos, positive| {
        let id = state.world.block_at(pos.x, pos.y, pos.z)?;
        let block = state.registry.blocks.by_id(mc_world::BlockStateId(id))?;
        mc_data::collision_shapes::vanilla_collision_shapes()
            .get_for_state(id, &block.block.id, &block.properties)
            .map(|shape| shape.face_quadrants(axis, positive))
    };
    match (face(from, positive), face(to, !positive)) {
        (Some(a), Some(b)) => a | b != 15,
        _ => false,
    }
}

pub(crate) fn waterlogged(state: &TickState, block: u32, wet: bool) -> Option<u32> {
    let entry = state.registry.blocks.by_id(mc_world::BlockStateId(block))?;
    if entry
        .properties
        .iter()
        .any(|(key, value)| key == "type" && value == "double")
    {
        return None;
    }
    let mut props = entry.properties.clone();
    let (_, value) = props.iter_mut().find(|(key, _)| key == "waterlogged")?;
    if (*value == "true") == wet {
        return None;
    }
    *value = wet.to_string();
    state
        .registry
        .blocks
        .by_name_and_props(&entry.block.id, &props)
        .map(|id| id.0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub enum Fluid {
    Water,
    Lava,
}

impl Fluid {
    fn from_id(id: &Identifier) -> Option<Self> {
        mc_world::ScheduledFluidTick::fluid_kind(id).map(|kind| match kind {
            FluidKind::Water => Self::Water,
            FluidKind::Lava => Self::Lava,
        })
    }

    /// Sources (including waterlogged blocks) and flowing states have distinct tick IDs.
    fn tick_id(self, level: u8) -> &'static Identifier {
        static WATER: LazyLock<Identifier> =
            LazyLock::new(|| Identifier::parse("minecraft:water").expect("static fluid id"));
        static FLOWING_WATER: LazyLock<Identifier> = LazyLock::new(|| {
            Identifier::parse("minecraft:flowing_water").expect("static fluid id")
        });
        static LAVA: LazyLock<Identifier> =
            LazyLock::new(|| Identifier::parse("minecraft:lava").expect("static fluid id"));
        static FLOWING_LAVA: LazyLock<Identifier> =
            LazyLock::new(|| Identifier::parse("minecraft:flowing_lava").expect("static fluid id"));
        match (self, level == 0) {
            (Self::Water, true) => &WATER,
            (Self::Water, false) => &FLOWING_WATER,
            (Self::Lava, true) => &LAVA,
            (Self::Lava, false) => &FLOWING_LAVA,
        }
    }

    /// Vanilla fluid tick delay for the owning dimension.
    fn delay(self, dimension: crate::dimensions::Dimension) -> u64 {
        match self {
            Self::Water => 5,
            Self::Lava if dimension == crate::dimensions::Dimension::Nether => 10,
            Self::Lava => 30,
        }
    }

    /// Vanilla horizontal fluid level falloff for the owning dimension.
    fn drop_off(self, dimension: crate::dimensions::Dimension) -> u8 {
        match self {
            Self::Water => 1,
            Self::Lava if dimension == crate::dimensions::Dimension::Nether => 1,
            Self::Lava => 2,
        }
    }

    fn levels(self, states: &ResolvedStates) -> &[u32; 16] {
        match self {
            Self::Water => &states.water_levels,
            Self::Lava => &states.lava_levels,
        }
    }

    fn source(self, states: &ResolvedStates) -> u32 {
        self.levels(states)[0]
    }

    fn falling(self, states: &ResolvedStates) -> u32 {
        self.levels(states)[8]
    }

    /// Flowing state holding `amount` (1-7).
    fn flowing(self, states: &ResolvedStates, amount: u8) -> u32 {
        self.levels(states)[usize::from(8 - amount.clamp(1, 7))]
    }
}

/// Fluid and `level` of a fluid block state.
pub fn fluid_at(states: &ResolvedStates, state: u32) -> Option<(Fluid, u8)> {
    [Fluid::Water, Fluid::Lava].into_iter().find_map(|fluid| {
        fluid
            .levels(states)
            .iter()
            .position(|level| *level == state)
            .map(|level| (fluid, level as u8))
    })
}

/// Vanilla `getAmount`: sources and falling fluid hold 8.
fn amount(level: u8) -> u8 {
    if level == 0 || level >= 8 {
        8
    } else {
        8 - level
    }
}

pub(crate) fn hydrate(state: &mut TickState, chunks: &[crate::boundary::ChunkPos]) {
    for pos in chunks {
        if let Some(chunk) = state.world.chunk(*pos) {
            state.fluid_ticks.hydrate(chunk, state.tick);
        }
    }
}

const SIDES: [[i32; 3]; 4] = [[1, 0, 0], [-1, 0, 0], [0, 0, 1], [0, 0, -1]];

fn offset(pos: BlockPos, step: [i32; 3]) -> BlockPos {
    BlockPos {
        x: pos.x + step[0],
        y: pos.y + step[1],
        z: pos.z + step[2],
    }
}

/// Schedule the fluid cells at and around every written position.
pub fn schedule(state: &mut TickState, writes: &[(BlockPos, u32)]) {
    for (pos, _) in writes {
        for step in [
            [0, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            let cell = offset(*pos, step);
            let Some(current) = state.world.block_at(cell.x, cell.y, cell.z) else {
                continue;
            };
            let Some((fluid, level)) = contained(state, current) else {
                continue;
            };
            state.fluid_ticks.schedule(
                cell,
                fluid.tick_id(level),
                state.tick + fluid.delay(state.dimension),
                0,
            );
        }
    }
}

/// Run the fluid ticks due now. New states go to the world and `writes`;
/// blocks the flow washes away drop their loot into `spawned`.
pub fn run(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, spawned: &mut Vec<EntityId>) {
    let due = state.fluid_ticks.drain(state.tick, &state.world);
    for tick in due {
        let pos = BlockPos {
            x: tick.pos[0],
            y: tick.pos[1],
            z: tick.pos[2],
        };
        if state
            .world
            .block_at(pos.x, pos.y, pos.z)
            .and_then(|block| contained(state, block))
            .is_some_and(|(fluid, level)| fluid.tick_id(level) == &tick.fluid)
        {
            tick_cell(state, pos, writes, spawned);
        }
    }
}

fn set(state: &mut TickState, writes: &mut Vec<(BlockPos, u32)>, pos: BlockPos, new: u32) {
    state.world.set_block(pos.x, pos.y, pos.z, new);
    writes.retain(|(written, _)| *written != pos);
    writes.push((pos, new));
}

/// Reconcile a source-water cell with the bubble producer immediately below it.
/// Writes schedule the next cell above through the ordinary fluid queue.
fn update_bubble_column(state: &TickState, pos: BlockPos, current: u32) -> Option<u32> {
    let entry = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(current))?;
    let is_column = entry.block.id.path() == "bubble_column";
    if !is_column && current != state.registry.states.water_levels[0] {
        return None;
    }
    let below = offset(pos, [0, -1, 0]);
    let support = state.world.block_at(below.x, below.y, below.z)?;
    let support = state
        .registry
        .blocks
        .by_id(mc_world::BlockStateId(support))?;
    let drag = match support.block.id.path() {
        "soul_sand" => Some(false),
        "magma_block" => Some(true),
        "bubble_column" => support
            .properties
            .iter()
            .find(|(key, _)| key == "drag")
            .map(|(_, value)| value == "true"),
        _ => None,
    };
    let Some(drag) = drag else {
        return is_column.then_some(state.registry.states.water_levels[0]);
    };
    static ID: LazyLock<Identifier> =
        LazyLock::new(|| Identifier::parse("minecraft:bubble_column").expect("static block id"));
    static UP: LazyLock<[(String, String); 1]> =
        LazyLock::new(|| [("drag".into(), "false".into())]);
    static DOWN: LazyLock<[(String, String); 1]> =
        LazyLock::new(|| [("drag".into(), "true".into())]);
    state
        .registry
        .blocks
        .by_name_and_props(&ID, if drag { &*DOWN } else { &*UP })
        .map(|id| id.0)
}

/// Vanilla `FlowingFluid.tick` for one cell.
fn tick_cell(
    state: &mut TickState,
    pos: BlockPos,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let states = state.registry.states;
    let Some(current) = state.world.block_at(pos.x, pos.y, pos.z) else {
        return;
    };
    let Some((fluid, level)) = contained(state, current) else {
        return;
    };
    if fluid == Fluid::Water
        && level == 0
        && let Some(new) = update_bubble_column(state, pos, current)
        && new != current
    {
        set(state, writes, pos, new);
    }
    // Vanilla `LiquidBlock.shouldSpreadLiquid`: lava touching water (not
    // from below) hardens.
    if fluid == Fluid::Lava {
        let touches_water = [[0, 1, 0], SIDES[0], SIDES[1], SIDES[2], SIDES[3]]
            .into_iter()
            .any(|step| {
                let cell = offset(pos, step);
                state
                    .world
                    .block_at(cell.x, cell.y, cell.z)
                    .and_then(|block| contained(state, block))
                    .is_some_and(|(other, _)| other == Fluid::Water)
            });
        if touches_water {
            let hard = if level == 0 {
                states.obsidian
            } else {
                states.cobblestone
            };
            set(state, writes, pos, hard);
            return;
        }
    }
    if fluid == Fluid::Water {
        for step in [[0, -1, 0], SIDES[0], SIDES[1], SIDES[2], SIDES[3]] {
            let cell = offset(pos, step);
            if let Some(block) = state.world.block_at(cell.x, cell.y, cell.z)
                && let Some(hardened) = crate::falling::concrete(&state.registry.blocks, block)
            {
                set(state, writes, cell, hardened);
            }
        }
    } else {
        crate::fire::lava_tick(state, pos, writes);
        if crate::fire::has_lava_fuel(state, pos) {
            state.fluid_ticks.schedule(
                pos,
                fluid.tick_id(level),
                state.tick + fluid.delay(state.dimension),
                0,
            );
        }
    }
    let mut level = level;
    if level != 0 {
        let new = new_liquid(state, pos, fluid);
        if new != current {
            set(state, writes, pos, new);
        }
        match fluid_at(&states, new) {
            Some((_, new_level)) => level = new_level,
            None => return,
        }
    }
    spread(state, pos, fluid, level, writes, spawned);
}

/// Vanilla `getNewLiquid`: what a non-source cell becomes from its
/// neighbours (air when nothing feeds it).
fn new_liquid(state: &TickState, pos: BlockPos, fluid: Fluid) -> u32 {
    let states = &state.registry.states;
    let fluid_of = |cell: BlockPos| {
        state
            .world
            .block_at(cell.x, cell.y, cell.z)
            .and_then(|block| contained(state, block))
            .filter(|(other, _)| *other == fluid)
    };
    let mut max = 0;
    let mut sources = 0;
    for step in SIDES {
        if !passage(state, offset(pos, step), pos) {
            continue;
        }
        if let Some((_, level)) = fluid_of(offset(pos, step)) {
            if level == 0 {
                sources += 1;
            }
            max = max.max(amount(level));
        }
    }
    let converts = crate::command_rules::boolean(
        state,
        if fluid == Fluid::Water {
            crate::command_rules::BoolRule::WATER_SOURCE_CONVERSION
        } else {
            crate::command_rules::BoolRule::LAVA_SOURCE_CONVERSION
        },
    );
    if converts && sources >= 2 {
        let below = offset(pos, [0, -1, 0]);
        let solid_below = state
            .world
            .block_at(below.x, below.y, below.z)
            .and_then(|block| state.registry.blocks.by_id(mc_world::BlockStateId(block)))
            .and_then(|entry| {
                mc_data::collision_shapes::vanilla_collision_shapes().get_for_state(
                    entry.id.0,
                    &entry.block.id,
                    &entry.properties,
                )
            })
            .is_some_and(|shape| !shape.is_empty());
        if solid_below || fluid_of(below).is_some_and(|(_, level)| level == 0) {
            return fluid.source(states);
        }
    }
    if fluid_of(offset(pos, [0, 1, 0])).is_some() && passage(state, offset(pos, [0, 1, 0]), pos) {
        return fluid.falling(states);
    }
    let left = max.saturating_sub(fluid.drop_off(state.dimension));
    if left == 0 {
        states.air
    } else {
        fluid.flowing(states, left)
    }
}

/// What flowing into `pos` does: `None` when the cell holds it back.
pub(crate) enum Flow {
    /// Air or the same fluid: write the fluid.
    Open,
    /// A non-colliding block (grass, flowers, torches): washed away.
    Wash(u32),
}

pub(crate) fn flow_into(state: &TickState, pos: BlockPos, fluid: Fluid) -> Option<Flow> {
    let block = state.world.block_at(pos.x, pos.y, pos.z)?;
    let states = &state.registry.states;
    if block == states.air {
        return Some(Flow::Open);
    }
    if let Some((other, level)) = fluid_at(states, block) {
        return (other == fluid && level != 0).then_some(Flow::Open);
    }
    let entry = state.registry.blocks.by_id(mc_world::BlockStateId(block))?;
    // A bubble column is a full water source, not a washable decoration.
    if entry.block.id.path() == "bubble_column" {
        return None;
    }
    // Material classification defaults unlisted states to solid. Use the
    // recorded collision shape instead, but never replace a waterloggable block.
    if entry.properties.iter().any(|(key, _)| key == "waterlogged") {
        return None;
    }
    let shape = mc_data::collision_shapes::vanilla_collision_shapes().get_for_state(
        block,
        &entry.block.id,
        &entry.properties,
    )?;
    shape.is_empty().then_some(Flow::Wash(block))
}

/// Write `new` into `pos`, washing away whatever stood there.
fn flow_to(
    state: &mut TickState,
    pos: BlockPos,
    into: Flow,
    new: u32,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let new = match into {
        Flow::Wash(washed) => {
            if !state.pop_block_loot(pos, washed, spawned) {
                return;
            }
            new
        }
        Flow::Open => new,
    };
    set(state, writes, pos, new);
}

/// Search at most four horizontal steps (two for slow Overworld lava).
/// The fixed recursion depth bounds work; reverse edges are excluded.
fn slope_distance(
    state: &TickState,
    pos: BlockPos,
    from: BlockPos,
    fluid: Fluid,
    depth: u8,
    limit: u8,
) -> u8 {
    let below = offset(pos, [0, -1, 0]);
    if passage(state, pos, below) && flow_into(state, below, fluid).is_some() {
        return depth;
    }
    if depth == limit {
        return u8::MAX;
    }
    let mut best = u8::MAX;
    for step in SIDES {
        let side = offset(pos, step);
        if side != from && passage(state, pos, side) && flow_into(state, side, fluid).is_some() {
            best = best.min(slope_distance(state, side, pos, fluid, depth + 1, limit));
        }
    }
    best
}

/// Vanilla `FlowingFluid.spread`: down first; sideways only from a source
/// or when nothing below takes the flow.
fn spread(
    state: &mut TickState,
    pos: BlockPos,
    fluid: Fluid,
    level: u8,
    writes: &mut Vec<(BlockPos, u32)>,
    spawned: &mut Vec<EntityId>,
) {
    let states = state.registry.states;
    let below = offset(pos, [0, -1, 0]);
    let below_block = state.world.block_at(below.x, below.y, below.z);
    let below_fluid = below_block.and_then(|block| contained(state, block));
    // Lava flowing down onto water makes stone.
    if fluid == Fluid::Lava
        && below_block
            .and_then(|block| fluid_at(&states, block))
            .is_some_and(|(other, _)| other == Fluid::Water)
        && passage(state, pos, below)
    {
        set(state, writes, below, states.stone);
        return;
    }
    if passage(state, pos, below)
        && let Some(into) = flow_into(state, below, fluid)
    {
        let falling = fluid.falling(&states);
        if below_block != Some(falling) {
            flow_to(state, below, into, falling, writes, spawned);
        }
        return;
    }
    // Resting on its own fluid: only a source spreads sideways.
    if level != 0 && below_fluid.is_some_and(|(other, _)| other == fluid) {
        return;
    }
    let left = amount(level).saturating_sub(fluid.drop_off(state.dimension));
    if left == 0 {
        return;
    }
    let new = fluid.flowing(&states, left);
    let limit = if fluid == Fluid::Lava && state.dimension != crate::dimensions::Dimension::Nether {
        2
    } else {
        4
    };
    let costs = SIDES.map(|step| {
        let side = offset(pos, step);
        if passage(state, pos, side) && flow_into(state, side, fluid).is_some() {
            slope_distance(state, side, pos, fluid, 0, limit)
        } else {
            u8::MAX
        }
    });
    let best = costs.into_iter().min().unwrap_or(u8::MAX);
    for (index, step) in SIDES.into_iter().enumerate() {
        let side = offset(pos, step);
        if costs[index] != best || !passage(state, pos, side) {
            continue;
        }
        let Some(into) = flow_into(state, side, fluid) else {
            continue;
        };
        let current = state.world.block_at(side.x, side.y, side.z);
        let weaker = current
            .and_then(|block| fluid_at(&states, block))
            .is_none_or(|(_, side_level)| amount(side_level) < left);
        if weaker {
            flow_to(state, side, into, new, writes, spawned);
        }
    }
}

/// The bucket action of a `UseItem`: the cell to rewrite and its new
/// state, or `None` when the bucket does nothing.
///
/// Vanilla `BucketItem.use`: clip block OUTLINE shapes and SOURCE_ONLY
/// fluids for pickup, or no fluids for pouring. A full bucket uses the hit
/// face's adjacent cell unless its hit block accepts the fluid. Reach 4.5.
pub fn bucket_target(
    state: &TickState,
    eye: [f64; 3],
    yaw: f32,
    pitch: f32,
    carried: Option<Fluid>,
) -> Option<(BlockPos, u32)> {
    let states = &state.registry.states;
    let hit = crate::pov_ray::clip(state, eye, yaw, pitch, carried.is_none())?;
    let pos = hit.pos;
    let block = state.world.block_at(pos.x, pos.y, pos.z)?;
    if let Some(fluid) = carried {
        if fluid == Fluid::Water
            && let Some(wet) = waterlogged(state, block, true)
        {
            return Some((pos, wet));
        }
        if let Some(Flow::Wash(_)) = flow_into(state, pos, fluid) {
            return Some((pos, fluid.source(states)));
        }
        return bucket_previous(state, offset(pos, hit.face), fluid);
    }
    contained(state, block)
        .is_some_and(|(_, level)| level == 0)
        .then(|| (pos, waterlogged(state, block, false).unwrap_or(states.air)))
}

#[path = "fluid_buckets.rs"]
mod buckets;
pub(crate) use buckets::plan_use_on;
pub(crate) use buckets::use_on;

/// The selected adjacent cell retains a waterlogged block or washes a replaceable block.
pub(crate) fn bucket_previous(
    state: &TickState,
    previous: BlockPos,
    fluid: Fluid,
) -> Option<(BlockPos, u32)> {
    let states = &state.registry.states;
    let open = state.world.block_at(previous.x, previous.y, previous.z)?;
    if fluid == Fluid::Water
        && let Some(wet) = waterlogged(state, open, true)
    {
        return Some((previous, wet));
    }
    let replaceable = open == states.air
        || fluid_at(states, open).is_some_and(|(_, level)| level != 0)
        || matches!(flow_into(state, previous, fluid), Some(Flow::Wash(_)));
    replaceable.then_some((previous, fluid.source(states)))
}
