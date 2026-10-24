//! Child dimension captures join the single native checkpoint transaction.
use super::Dimension;
use crate::core::CheckpointPlayerData;
use crate::journal::CheckpointManifest;
use crate::tick::TickState;
use std::collections::{BTreeMap, BTreeSet};

pub(crate) struct Capture {
    pub regions: crate::world::ChunkCapture,
    pub players: Vec<CheckpointPlayerData>,
    pub entities: BTreeMap<(i32, i32), mc_world::anvil::EntityChunk>,
    pub spawn: [i32; 3],
    pub rotation: [f32; 2],
    pub claims: BTreeSet<String>,
    pub border: crate::command_border::Border,
    pub tickets: BTreeMap<String, ([i32; 2], (u64, i32))>,
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    pub fluid_ticks: crate::fluids::FluidTicksImage,
    pub inhabited_time: Vec<crate::journal::InhabitedTimeImage>,
    pub debt: Vec<crate::journal::SimDebtImage>,
    pub end_fight: Option<crate::end_fight::Fight>,
}

pub(crate) struct Ack {
    pub written: Vec<(crate::boundary::ChunkPos, u64)>,
    pub block_ticks: crate::block_ticks::BlockTicksImage,
    pub fluid_ticks: crate::fluids::FluidTicksImage,
}

pub(crate) type Written = BTreeMap<Dimension, Vec<(crate::boundary::ChunkPos, u64)>>;

pub(crate) fn capture(state: &mut TickState, tick: u64) -> BTreeMap<Dimension, Capture> {
    state
        .other_dimensions
        .iter_mut()
        .map(|(&dimension, world)| {
            let (players, entities, block_entities) = crate::core::checkpoint_data_for(world, tick);
            crate::campfire::checkpoint(world);
            crate::bees::checkpoint(world);
            crate::jukebox::checkpoint(world);
            crate::brewing::checkpoint(world);
            crate::sculk::checkpoint(world);
            crate::crafter::checkpoint(world);
            crate::shelf::checkpoint(world);
            crate::shulker::checkpoint(world);
            crate::redstone::containers::checkpoint(world);
            crate::end_portals::checkpoint(world);
            let absent_blocks = world.block_ticks.checkpoint(&mut world.world, tick);
            let absent_fluids = world.fluid_ticks.checkpoint(&mut world.world, tick);
            let mut regions = world.world.capture_dirty(&block_entities);
            world
                .world
                .capture_scheduled_block_ticks(&mut regions, absent_blocks);
            world
                .world
                .capture_scheduled_fluid_ticks(&mut regions, absent_fluids);
            (
                dimension,
                Capture {
                    regions,
                    players,
                    entities,
                    spawn: world.world.world_spawn(),
                    rotation: world.world.world_spawn_rotation(),
                    claims: world.generated_entity_claims.clone(),
                    border: world.border,
                    tickets: crate::core::checkpoint_tickets(world, tick),
                    block_ticks: world.block_ticks.image(),
                    fluid_ticks: world.fluid_ticks.image(),
                    inhabited_time: world.world.inhabited_checkpoint(tick),
                    debt: world.world.debt_checkpoint(tick),
                    end_fight: (dimension == Dimension::End).then(|| world.end_fight.clone()),
                },
            )
        })
        .collect()
}

pub(crate) fn images(
    captures: &BTreeMap<Dimension, Capture>,
    tick: u64,
) -> BTreeMap<Dimension, Box<CheckpointManifest>> {
    let mut manifests = BTreeMap::new();
    for (&dimension, capture) in captures {
        manifests.insert(
            dimension,
            Box::new(CheckpointManifest {
                tick,
                domain_versions: crate::native_checkpoint::domain_versions(),
                dimensions: BTreeMap::new(),
                players: BTreeMap::new(),
                world_spawn: Some(capture.spawn),
                world_spawn_rotation: capture.rotation,
                generated_entity_claims: capture.claims.clone(),
                tickets: capture.tickets.clone(),
                block_ticks: capture.block_ticks.clone(),
                fluid_ticks: capture.fluid_ticks.clone(),
                inhabited_time: capture.inhabited_time.clone(),
                debt: capture.debt.clone(),
                climate: None,
                border: capture.border,
                // Shared difficulty is recovered from the root authority before ticking.
                difficulty: Default::default(),
                gamerules: Default::default(),
                end_fight: capture.end_fight.clone(),
                raids: None,
                wandering_trader: None,
                scheduled_functions: Vec::new(),
                datapack_selection: None,
            }),
        );
    }
    manifests
}

pub(crate) fn acknowledge(state: &mut TickState, acks: BTreeMap<Dimension, Ack>) {
    for (dimension, ack) in acks {
        if let Some(world) = state.other_dimensions.get_mut(&dimension) {
            world.world.mark_flushed(&ack.written);
            world.block_ticks.acknowledge_flushed(&ack.block_ticks);
            world.fluid_ticks.acknowledge_flushed(&ack.fluid_ticks);
        }
    }
}

pub(crate) fn take_generated(
    state: &mut TickState,
    limit: usize,
) -> Option<crate::world::ChunkCapture> {
    state.world.take_generated(limit).or_else(|| {
        state
            .other_dimensions
            .values_mut()
            .find_map(|world| world.world.take_generated(limit))
    })
}
