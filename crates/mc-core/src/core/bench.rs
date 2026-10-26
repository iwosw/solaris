//! Container benchmark harness.
//! Moved verbatim from `core.rs`.

use super::*;

/// Memory-only container benchmark owner; never starts the paced tick thread.
#[doc(hidden)]
pub struct ContainerBench {
    pub(super) state: TickState,
}

pub(super) struct ContainerBenchGenerator {
    pub(super) air: mc_world::BlockStateId,
}

impl ChunkGenerator for ContainerBenchGenerator {
    fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
        mc_world::Chunk::empty(
            pos,
            self.air,
            mc_data::Identifier::parse("minecraft:plains").expect("plains identifier"),
        )
    }
}

impl ContainerBench {
    pub const CHESTS: usize = 20_000;
    pub const COMPARATORS: usize = 4;
    const COLUMNS: usize = 200;
    const Y: i32 = 200;

    pub fn new(vanilla_data_dir: &std::path::Path) -> Result<Self, CoreError> {
        let registry = Arc::new(CoreRegistry::load(Some(vanilla_data_dir))?);
        let air = mc_world::BlockStateId(registry.states.air);
        let generator = Arc::new(ContainerBenchGenerator { air });
        let world = ChunkWorld::new(
            42,
            generator,
            &registry,
            air,
            [0, Self::Y, 0],
            mc_world::OVERWORLD_GEOMETRY,
        );
        let mut state = TickState::new(registry, world, 2, 512);
        // Exact rectangle, including the comparators at x=-1. Zero-radius
        // tickets retain every scene chunk without players or new worldgen.
        for x in -1..=24 {
            for z in 0..=12 {
                state.tickets.insert(
                    format!("containers-{x}-{z}"),
                    (crate::boundary::ChunkPos { x, z }, 0),
                );
            }
        }
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        if let Some(error) = state.world.load_errors().next() {
            return Err(CoreError::Storage(error.to_string()));
        }
        let chest = Self::block(&state, "chest", &[]);
        let comparator = Self::block(
            &state,
            "comparator",
            &[
                ("facing", "east"),
                ("mode", "compare"),
                ("powered", "false"),
            ],
        );
        let stone = Self::block(&state, "stone", &[]);
        let item = state
            .registry
            .items
            .id_of(&mc_data::Identifier::parse("minecraft:stone").expect("stone identifier"))
            .expect("stone item");
        for index in 0..Self::CHESTS {
            let pos = Self::chest_pos(index);
            state.world.set_block(pos.x, pos.y, pos.z, chest);
            state.block_entities.insert(
                (pos.x, pos.y, pos.z),
                crate::tick::BlockEntityState {
                    slots: vec![mc_data::ItemStack::new(item, 64); 27],
                    furnace: None,
                },
            );
        }
        // Each comparator reads the first chest of a different row; no wire
        // network, automation or shared double chest hides the signal cost.
        for index in 0..Self::COMPARATORS {
            let pos = Self::comparator_pos(index);
            state.world.set_block(pos.x, pos.y - 1, pos.z, stone);
            state.world.set_block(pos.x, pos.y, pos.z, comparator);
        }
        state.world.fold_overlay();
        Ok(Self { state })
    }

    pub(super) fn block(state: &TickState, name: &str, props: &[(&str, &str)]) -> u32 {
        let id = mc_data::Identifier::parse(format!("minecraft:{name}"))
            .expect("benchmark block identifier");
        let base = state
            .registry
            .blocks
            .block(&id)
            .expect("benchmark block")
            .default;
        let mut values = state
            .registry
            .blocks
            .by_id(base)
            .expect("benchmark block state")
            .properties
            .clone();
        for (key, value) in props {
            values
                .iter_mut()
                .find(|(k, _)| k == key)
                .expect("block property")
                .1 = (*value).into();
        }
        state
            .registry
            .blocks
            .by_name_and_props(&id, &values)
            .expect("block properties")
            .0
    }

    pub(super) fn chest_pos(index: usize) -> crate::boundary::BlockPos {
        crate::boundary::BlockPos {
            x: (index % Self::COLUMNS) as i32 * 2,
            y: Self::Y,
            z: (index / Self::COLUMNS) as i32 * 2,
        }
    }

    pub(super) fn comparator_pos(index: usize) -> crate::boundary::BlockPos {
        let mut pos = Self::chest_pos(index * Self::COLUMNS);
        pos.x -= 1;
        pos
    }

    /// Four inventory writes per tick, outside the timed region. All slots
    /// remain nonempty; alternating full/quarter-full changes analog strength.
    pub fn mutate(&mut self, tick: usize) {
        let count = if tick.is_multiple_of(2) { 16 } else { 64 };
        for index in 0..Self::COMPARATORS {
            let pos = Self::chest_pos(index * Self::COLUMNS);
            let record = self
                .state
                .block_entities
                .get_mut(&(pos.x, pos.y, pos.z))
                .expect("benchmark chest remains live");
            for stack in &mut record.slots {
                stack.count = count;
            }
        }
    }

    /// Measures the real dimension-coordinated run_tick, including publication
    /// and all ordinary phases, but not pacing, journal I/O or output disposal.
    pub fn tick(&mut self) -> TickTimes {
        let start = std::time::Instant::now();
        let (_output, _drains, mut times) = run_tick(&mut self.state, &[]);
        times.total_us = start.elapsed().as_micros() as u64;
        times
    }

    pub fn loaded_chunks(&self) -> usize {
        self.state.world.chunks().count()
    }

    /// Untimed scene fence: loaded storage must stay nonempty and settled
    /// comparator NBT must match the four mutated inventories.
    pub fn assert_scene(&self) -> u8 {
        assert_eq!(self.state.block_entities.len(), Self::CHESTS);
        for index in 0..Self::CHESTS {
            let pos = Self::chest_pos(index);
            let record = self
                .state
                .block_entities
                .get(&(pos.x, pos.y, pos.z))
                .expect("all benchmark chests remain live");
            assert_eq!(record.slots.len(), 27);
            assert!(record.slots.iter().all(|stack| !stack.is_empty()));
            let block = self
                .state
                .world
                .block_at(pos.x, pos.y, pos.z)
                .and_then(|id| self.state.registry.blocks.by_id(mc_world::BlockStateId(id)))
                .expect("all benchmark chests remain loaded");
            assert_eq!(block.block.id.path(), "chest");
        }
        let mut level = 0;
        for index in 0..Self::COMPARATORS {
            let chest = Self::chest_pos(index * Self::COLUMNS);
            let count = self.state.block_entities[&(chest.x, chest.y, chest.z)].slots[0].count;
            level = if count == 64 { 15 } else { 4 };
            let pos = Self::comparator_pos(index);
            let chunk = self
                .state
                .world
                .chunk(crate::boundary::ChunkPos {
                    x: pos.x.div_euclid(16),
                    z: pos.z.div_euclid(16),
                })
                .expect("comparator remains loaded");
            let bytes = chunk
                .block_entities
                .get(&mc_world::BlockPos {
                    x: pos.x,
                    y: pos.y,
                    z: pos.z,
                })
                .expect("comparator has committed analog output");
            let mc_nbt::Tag::Compound(fields) =
                mc_nbt::read_network(&mut bytes.as_slice()).expect("comparator NBT")
            else {
                panic!("comparator NBT must be a compound");
            };
            let output = fields.iter().find_map(|(name, tag)| {
                if name == "OutputSignal"
                    && let mc_nbt::Tag::Int(value) = tag
                {
                    Some(*value)
                } else {
                    None
                }
            });
            assert_eq!(
                output,
                Some(i32::from(level)),
                "comparator {index} analog output"
            );
        }
        level
    }
}
