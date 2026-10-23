//! Horde entity-phase benchmark scene (`core-bench horde100|horde1000`) and the
//! flat test world shared with the tick tests.
//!
//! One flat arena, fully resident through zero-radius tickets: 10k zombies and
//! 5k villagers plus 100 or 1000 invulnerable survival players at seeded
//! uniform positions, at night, without natural spawning. It drives the real
//! `run_tick` (never the paced thread), so `entity_us` is the ordinary entity
//! phase including chaser target selection.

use std::sync::Arc;

use crate::boundary::{ChunkPos, ConnectionId, EntityId, PlayerInput, TickTimes};
use crate::config::CoreError;
use crate::registry::CoreRegistry;
use crate::tick::{TickState, run_tick};
use crate::world::ChunkWorld;

#[cfg(test)]
#[path = "merge_cost_tests.rs"]
pub(crate) mod merge_cost_tests;

/// Flat, prelit terrain for tests of gameplay rather than world generation.
/// Each loader still produces its own owned chunk, but identical columns need
/// neither repeated terrain routing nor repeated isolated light baking.
pub(crate) fn flat_world(registry: &CoreRegistry) -> ChunkWorld {
    struct FlatTerrain(mc_world::Chunk);
    impl mc_world::ChunkGenerator for FlatTerrain {
        fn generate(&self, pos: mc_world::ChunkPos) -> mc_world::Chunk {
            let mut chunk = self.0.clone();
            chunk.pos = pos;
            chunk
        }
    }
    let mut template = mc_world::Chunk::empty(
        mc_world::ChunkPos { x: 0, z: 0 },
        mc_world::BlockStateId(registry.states.air),
        mc_data::Identifier::parse("minecraft:plains").unwrap(),
    );
    for x in 0..16 {
        for z in 0..16 {
            for (y, block) in [
                (61, registry.states.stone),
                (62, registry.states.dirt),
                (63, registry.states.grass_block),
            ] {
                template.set_block(x, y, z, mc_world::BlockStateId(block));
            }
        }
    }
    template.status = "minecraft:full".into();
    crate::light::bake_missing(&mut template, &registry.light);
    ChunkWorld::new(
        42,
        Arc::new(FlatTerrain(template)),
        registry,
        mc_world::BlockStateId(registry.states.air),
        [8, 64, 8],
        mc_world::OVERWORLD_GEOMETRY,
    )
}

/// Memory-only horde benchmark owner; never starts the paced tick thread.
#[doc(hidden)]
pub struct HordeBench {
    state: TickState,
    players: Vec<EntityId>,
    /// Mobs whose movement the scene fence checks, with their start position.
    watched: Vec<(EntityId, [f64; 3])>,
    /// Seeded `(zombies, villagers, cows)`.
    counts: (usize, usize, usize),
}

/// splitmix64: the scene layout is a pure function of its seed.
struct Layout(u64);

impl Layout {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    fn position(&mut self) -> [f64; 3] {
        let unit = |bits: u64| (bits >> 11) as f64 / (1u64 << 53) as f64;
        [
            unit(self.next()) * HordeBench::SIDE as f64,
            HordeBench::FLOOR_Y,
            unit(self.next()) * HordeBench::SIDE as f64,
        ]
    }
}

impl HordeBench {
    /// Arena edge in blocks (a 32 x 32 chunk square).
    const SIDE: i32 = 512;
    /// Feet height above the flat floor (top block y=63).
    const FLOOR_Y: f64 = 64.0;
    const SEED: u64 = 0x4852_4F44_4531;

    /// 10k zombies and 5k villagers hunted by/among `players`.
    pub fn horde(players: usize, vanilla_data_dir: &std::path::Path) -> Result<Self, CoreError> {
        Self::build(players, 10_000, 5_000, 0, vanilla_data_dir)
    }

    /// No-regression scene without a single hostile mob: 50k villagers and
    /// 20k cows (no chaser ever searches for a target).
    pub fn peaceful(players: usize, vanilla_data_dir: &std::path::Path) -> Result<Self, CoreError> {
        Self::build(players, 0, 50_000, 20_000, vanilla_data_dir)
    }

    fn build(
        players: usize,
        zombies: usize,
        villagers: usize,
        cows: usize,
        vanilla_data_dir: &std::path::Path,
    ) -> Result<Self, CoreError> {
        let registry = Arc::new(CoreRegistry::load(Some(vanilla_data_dir))?);
        let world = flat_world(&registry);
        let mut state = TickState::new(Arc::clone(&registry), world, 2, 512);
        state.gamerules.insert("spawn_mobs".into(), "false".into());
        // Night: zombies never burn within the measured window.
        state.day_time = 14_000;
        for x in 0..Self::SIDE / 16 {
            for z in 0..Self::SIDE / 16 {
                state
                    .tickets
                    .insert(format!("horde-{x}-{z}"), (ChunkPos { x, z }, 0));
            }
        }
        let joins: Vec<_> = (1..=players as u64)
            .map(|connection| PlayerInput::Join {
                connection: ConnectionId(connection),
                uuid: uuid::Uuid::from_u128(u128::from(connection)),
                name: format!("horde{connection}"),
                permission_level: 0,
            })
            .collect();
        run_tick(&mut state, &joins);
        let mut layout = Layout(Self::SEED);
        let player_ids: Vec<EntityId> = (1..=players as u64)
            .map(|connection| {
                state
                    .connections
                    .entity_of(ConnectionId(connection))
                    .expect("bench player joined")
            })
            .collect();
        for id in &player_ids {
            let slot = state.entities.slot_of(*id).expect("bench player slot");
            state.entities.set_pos(slot, layout.position());
            // Targets stay targets: hits are swallowed, nobody dies mid-run.
            state.entities.row_mut(slot).protected_until = u64::MAX;
        }
        let want = state.desired_chunks();
        state.world.ensure_blocking(&want);
        if let Some(error) = state.world.load_errors().next() {
            return Err(CoreError::Storage(error.to_string()));
        }
        let type_id = |name: &str| {
            registry
                .entity_types
                .id_of(&mc_data::Identifier::parse(name).expect("bench entity identifier"))
                .expect("bench entity type")
        };
        let zombie_type = type_id("minecraft:zombie");
        let villager_type = type_id("minecraft:villager");
        let cow_type = type_id("minecraft:cow");
        let mut watched = Vec::new();
        for _ in 0..zombies {
            let at = layout.position();
            let id = state.spawn_mob(zombie_type, at, 20.0);
            let slot = state.entities.slot_of(id).expect("bench zombie slot");
            state.entities.row_mut(slot).persistent = true;
            watched.push((id, at));
        }
        for _ in 0..villagers {
            // Effectively unkillable: zombies keep hunting for the whole run.
            let id = state.spawn_mob(villager_type, layout.position(), 1.0e9);
            let slot = state.entities.slot_of(id).expect("bench villager slot");
            state.entities.row_mut(slot).persistent = true;
        }
        for _ in 0..cows {
            let at = layout.position();
            let id = state.spawn_mob(cow_type, at, 10.0);
            let slot = state.entities.slot_of(id).expect("bench cow slot");
            state.entities.row_mut(slot).persistent = true;
            if zombies == 0 {
                watched.push((id, at));
            }
        }
        Ok(Self {
            state,
            players: player_ids,
            watched,
            counts: (zombies, villagers, cows),
        })
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

    /// Untimed scene fence: the population is intact and the watched mobs
    /// (zombies, or cows in the peaceful scene) decided and moved: frozen or
    /// idle mobs would make the entity phase meaningless. Returns
    /// `(zombies, villagers, cows, players, moved_watched)`.
    pub fn assert_scene(&self) -> (usize, usize, usize, usize, usize) {
        let (mut zombies, mut villagers, mut cows) = (0, 0, 0);
        for slot in self.state.entities.slots() {
            let body = self.state.entities.body(slot);
            if !body.alive {
                continue;
            }
            if let crate::entities::EntityKind::Mob { type_id } = body.kind {
                match crate::piglin::name(type_id) {
                    "minecraft:zombie" => zombies += 1,
                    "minecraft:villager" => villagers += 1,
                    "minecraft:cow" => cows += 1,
                    _ => {}
                }
            }
        }
        assert_eq!(zombies, self.counts.0, "zombies must survive the run");
        assert_eq!(villagers, self.counts.1, "villagers must survive the run");
        assert_eq!(cows, self.counts.2, "cows must survive the run");
        assert_eq!(self.state.players.len(), self.players.len());
        let moved = self
            .watched
            .iter()
            .filter(|(id, at)| {
                self.state
                    .entities
                    .slot_of(*id)
                    .is_some_and(|slot| self.state.entities.pos(slot) != *at)
            })
            .count();
        // Zombies chase every decision; cows only wander now and then.
        let needed = self.watched.len() / if self.counts.0 > 0 { 2 } else { 10 };
        assert!(
            moved >= needed,
            "watched mobs must be deciding and moving: {moved} of {} moved",
            self.watched.len()
        );
        (zombies, villagers, cows, self.players.len(), moved)
    }
}
