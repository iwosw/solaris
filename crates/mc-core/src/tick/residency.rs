//! Active-chunk refresh, inhabited time and desired residency.
//! Moved verbatim from `tick.rs`.

use super::*;

impl TickState {
    /// Enter at the simulation radius; retain the previous set one chunk
    /// farther out so crossing a chunk boundary does not flap simulation.
    pub(crate) fn refresh_active_chunks(&mut self, tick: u64) {
        let radius = self.config_sim_distance.max(0);
        let entities = &self.entities;
        let centers = self.players.keys().filter_map(|id| {
            let pos = entities.pos(entities.slot_of(*id)?);
            Some(ChunkPos {
                x: (mc_physics::mth::floor_i32(pos[0])).div_euclid(16),
                z: (mc_physics::mth::floor_i32(pos[2])).div_euclid(16),
            })
        });
        if radius == self.active_sim_radius
            && centers
                .clone()
                .eq(self.active_player_centers.iter().copied())
            && self
                .ticket_areas()
                .map(|(center, _, blocks, entities)| (center, blocks, entities))
                .eq(self.active_ticket_ranges.iter().copied())
        {
            self.world.refresh_active_clock(tick);
            return;
        }
        self.active_player_centers.clear();
        self.active_player_centers.extend(centers);
        let ranges: Vec<_> = self
            .ticket_areas()
            .map(|(center, _, blocks, entities)| (center, blocks, entities))
            .collect();
        self.active_ticket_ranges = ranges;
        self.active_sim_radius = radius;
        let mut active = HashSet::default();
        for center in &self.active_player_centers {
            for z in center.z - radius..=center.z + radius {
                for x in center.x - radius..=center.x + radius {
                    active.insert(ChunkPos { x, z });
                }
            }
            for chunk in self.world.entity_active_positions() {
                if (i64::from(chunk.x) - i64::from(center.x)).abs() <= i64::from(radius) + 1
                    && (i64::from(chunk.z) - i64::from(center.z)).abs() <= i64::from(radius) + 1
                {
                    active.insert(chunk);
                }
            }
        }
        let mut block_only = HashSet::default();
        for (center, _, blocks, entities) in self.ticket_areas() {
            for z in center.z - blocks..=center.z + blocks {
                for x in center.x - blocks..=center.x + blocks {
                    block_only.insert(ChunkPos { x, z });
                }
            }
            for z in center.z - entities..=center.z + entities {
                for x in center.x - entities..=center.x + entities {
                    active.insert(ChunkPos { x, z });
                }
            }
        }
        block_only.retain(|chunk| !active.contains(chunk));
        self.world.set_block_only_chunks(block_only);
        self.world.set_active_chunks(active, tick);
    }

    /// Vanilla's spawning/ticking chunks: resident simulation chunks whose
    /// centres are less than 128 blocks from a non-spectator player.
    pub(super) fn accumulate_inhabited_time(&mut self) {
        // Accumulation precedes commit's tick increment; interval boundaries
        // must use the output/WAL tick, including the first tick after recovery.
        let tick = self.tick + 1;
        // Forced chunks within the spawning sphere count too, independently
        // of the player's configured simulation radius.
        let radius = 8;
        for (id, player) in &self.players {
            if player.spectator() {
                continue;
            }
            let Some(slot) = self.entities.slot_of(*id) else {
                continue;
            };
            let pos = self.entities.pos(slot);
            let cx = (mc_physics::mth::floor_i32(pos[0])).div_euclid(16);
            let cz = (mc_physics::mth::floor_i32(pos[2])).div_euclid(16);
            for z in cz - radius..=cz + radius {
                for x in cx - radius..=cx + radius {
                    let dx = f64::from(x) * 16.0 + 8.0 - pos[0];
                    let dz = f64::from(z) * 16.0 + 8.0 - pos[2];
                    if dx * dx + dz * dz < 128.0 * 128.0 && self.world.is_active(x, z) {
                        self.world.tick_inhabited_time(ChunkPos { x, z }, tick);
                    }
                }
            }
        }
        self.world.finish_inhabited_tick(tick);
    }

    /// The sole residency demand: per-player effective view (negotiated client
    /// radius, capped at the configured view, floored by the simulation
    /// radius) plus one chunk of margin, active chunks, explicit/respawn
    /// tickets and unacknowledged dirty pins. Unnegotiated players use the
    /// configured view. Load priorities prefer the nearest player/ticket
    /// centre; demands union across players.
    pub fn desired_chunks(&self) -> HashMap<ChunkPos, i32> {
        let mut want: HashMap<ChunkPos, i32> = HashMap::default();
        let mut add = |center: ChunkPos, radius: i32| {
            for z in center.z - radius..=center.z + radius {
                for x in center.x - radius..=center.x + radius {
                    let (dx, dz) = (x - center.x, z - center.z);
                    let priority = dx * dx + dz * dz;
                    want.entry(ChunkPos { x, z })
                        .and_modify(|best| *best = (*best).min(priority))
                        .or_insert(priority);
                }
            }
        };
        for id in self.players.keys() {
            let Some(slot) = self.entities.slot_of_any(*id) else {
                continue;
            };
            let pos = self.entities.pos(slot);
            let center = ChunkPos {
                x: (mc_physics::mth::floor_i32(pos[0])).div_euclid(16),
                z: (mc_physics::mth::floor_i32(pos[2])).div_euclid(16),
            };
            add(center, self.residency_radius_for(*id));
            if !self.entities.body(slot).alive {
                let spawn = self.world.world_spawn();
                add(
                    ChunkPos {
                        x: spawn[0].div_euclid(16),
                        z: spawn[2].div_euclid(16),
                    },
                    1,
                );
                if let Some(spawn) = self.players.get(id).and_then(|player| player.respawn_point) {
                    add(
                        ChunkPos {
                            x: spawn[0].div_euclid(16),
                            z: spawn[2].div_euclid(16),
                        },
                        1,
                    );
                }
            }
        }
        for (centre, load, _, _) in self.ticket_areas() {
            add(centre, load);
        }
        for chunk in self.world.active_positions().chain(self.world.dirty_pins()) {
            want.entry(chunk).or_insert(i32::MAX);
        }
        want
    }

    /// Effective residency radius for one joined player: negotiated client
    /// view capped at the configured view, floored by the configured
    /// simulation radius, plus one chunk of margin. Unnegotiated players
    /// use the configured view (so `max` keeps at least simulation).
    pub(super) fn residency_radius_for(&self, id: EntityId) -> i32 {
        let negotiated = self
            .connections
            .entity_connection(id)
            .and_then(|connection| self.connections.view_distance_of(connection))
            .map(i32::from);
        let effective_view = match negotiated {
            Some(view) => view.min(self.config_view_distance.max(0)),
            None => self.config_view_distance.max(0),
        }
        .max(self.config_sim_distance.max(0));
        effective_view.saturating_add(1)
    }
}
