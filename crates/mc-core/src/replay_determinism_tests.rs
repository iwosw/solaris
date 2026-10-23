//! Determinism canary (from the RnD-Replay probe): two independent worlds fed
//! the same `PlayerInput` sequence at the same tick numbers must hold identical
//! chunk residency, blocks, entities (including uuids) and player state on
//! every tick (the wall clock is the one injected external input). Nothing is
//! masked: random uuids or loader-ticket-shifted revision stamps would fail it.

use std::collections::{BTreeMap, BTreeSet};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::boundary::{BlockPos, ConnectionId, DigAction, EntityId, Hand, PlayerInput};
use crate::entities::EntitySlot;
use crate::tick::{TickState, run_tick};
use crate::tick_tests::full_registry;
use crate::world::ChunkWorld;

const TOTAL_TICKS: u64 = 350;
const VARIANT_TICK: u64 = 50;

fn fresh_state() -> TickState {
    let registry = Arc::new(full_registry());
    let terrain = mc_worldgen::TerrainGenerator::new(42, Arc::clone(&registry.blocks));
    let spawn = terrain.locate_safe_spawn().expect("test terrain spawn");
    let world_spawn = [spawn.block_x, spawn.surface_y + 1, spawn.block_z];
    let generator: Arc<dyn mc_world::ChunkGenerator> = Arc::new(terrain);
    let world = ChunkWorld::new(
        42,
        generator,
        &registry,
        mc_world::BlockStateId(registry.states.air),
        world_spawn,
        mc_world::OVERWORLD_GEOMETRY,
    );
    let mut state = TickState::new(registry, world, 2, 512);
    // The wall clock is an external input; pin it for the replay.
    state.wall_clock = || 1_767_225_600;
    state
}

fn h<T: Hash + ?Sized>(value: &T) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

// ------------------------------------------------------------------ script

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Variant {
    A,
    B,
}

fn cmd(connection: u64, command: &str) -> PlayerInput {
    PlayerInput::Command {
        connection: ConnectionId(connection),
        command: command.to_owned(),
        permission_level: 4,
    }
}

/// Scripted client. Timed commands are open-loop; walking, teleport
/// confirmation and the block-edit targets react to the *server's* state the
/// way a real client does. The probe records what this produces and then
/// replays the recording open-loop on the second world.
/// Variant only matters from `VARIANT_TICK` on.
fn script(i: u64, variant: Variant, state: &TickState) -> Vec<PlayerInput> {
    let c1 = ConnectionId(1);
    let c2 = ConnectionId(2);
    let mut out = Vec::new();
    let variant = if i < VARIANT_TICK {
        Variant::A
    } else {
        variant
    };
    let p1 = state.connections.entity_of(c1);
    let p1_pos = p1.and_then(|id| state.entities.slot_of(id)).map(|slot| {
        (
            state.entities.pos(slot),
            state.entities.ground(slot),
            state.entities.rot(slot),
        )
    });
    if i > 1 {
        for (conn, connection) in [(1u64, c1), (2, c2)] {
            let _ = conn;
            let awaiting = state
                .connections
                .entity_of(connection)
                .and_then(|id| state.players.get(&id))
                .and_then(|player| player.awaiting_teleport);
            if let Some(teleport_id) = awaiting {
                out.push(PlayerInput::ConfirmTeleport {
                    connection,
                    teleport_id,
                });
            }
        }
    }
    match i {
        0 => {
            for (conn, n) in [(1u64, 1u128), (2, 2)] {
                out.push(PlayerInput::Join {
                    connection: ConnectionId(conn),
                    uuid: uuid::Uuid::from_u128(0xA570_0000 + n),
                    name: format!("probe{conn}"),
                    permission_level: 4,
                });
            }
        }
        1 => {
            for c in [c1, c2] {
                out.push(PlayerInput::ConfirmTeleport {
                    connection: c,
                    teleport_id: 1,
                });
            }
        }
        2 => {
            out.push(cmd(1, "gamemode creative"));
            out.push(cmd(2, "gamemode survival"));
            out.push(cmd(1, "time set midnight"));
            out.push(cmd(1, "give @s minecraft:stone 64"));
            out.push(cmd(1, "give @s minecraft:diamond_sword 1"));
            out.push(cmd(2, "give @s minecraft:bread 8"));
        }
        3 => {
            out.push(cmd(1, "summon minecraft:zombie ~6 ~ ~"));
            out.push(cmd(1, "summon minecraft:cow ~-4 ~ ~3"));
            out.push(cmd(1, "summon minecraft:pig ~-4 ~ ~-3"));
            out.push(cmd(1, "summon minecraft:sheep ~2 ~ ~5"));
            out.push(cmd(1, "summon minecraft:villager ~3 ~ ~-5"));
            out.push(cmd(1, "summon minecraft:chicken ~5 ~ ~5"));
            out.push(cmd(1, "summon minecraft:skeleton ~-6 ~ ~-6"));
        }
        20 => out.push(cmd(1, "setblock ~3 ~5 ~ minecraft:sand")),
        25 => out.push(cmd(1, "setblock ~4 ~2 ~1 minecraft:water")),
        30 => out.push(cmd(1, "setblock ~-3 ~2 ~1 minecraft:lava")),
        40 => out.push(PlayerInput::DropSelected {
            connection: c1,
            whole_stack: false,
        }),
        _ => {}
    }
    // Walk (p1 every tick from 5), look around (p2), edit blocks every 7 ticks.
    if i >= 5
        && let Some((pos, on_ground, _)) = p1_pos
    {
        let (dx, dz) = match variant {
            Variant::A => (0.2, 0.0),
            Variant::B => (0.0, -0.2),
        };
        out.push(PlayerInput::Move {
            connection: c1,
            position: [pos[0] + dx, pos[1], pos[2] + dz],
            yaw: if variant == Variant::A { -90.0 } else { 180.0 },
            pitch: 0.0,
            on_ground,
        });
        out.push(PlayerInput::MoveRotation {
            connection: c2,
            yaw: (i as f32 * 3.0) % 360.0,
            pitch: 0.0,
            on_ground: true,
        });
    }
    if i >= 8
        && i.is_multiple_of(7)
        && let Some((pos, _, _)) = p1_pos
    {
        let k = (i / 7) as i32;
        let (ox, oz) = match variant {
            Variant::A => (2 + k % 5, k % 3),
            Variant::B => (-(k % 5) - 2, -(k % 4) - 1),
        };
        let (bx, bz) = (pos[0].floor() as i32 + ox, pos[2].floor() as i32 + oz);
        let air = state.registry.states.air;
        let gy = (-60..320)
            .rev()
            .find(|&y| state.world.block_at(bx, y, bz).is_some_and(|s| s != air))
            .unwrap_or(60);
        let pos = BlockPos {
            x: bx,
            y: gy,
            z: bz,
        };
        if k % 2 == 0 {
            out.push(PlayerInput::Dig {
                connection: c1,
                position: pos,
                action: DigAction::Start,
                sequence: i as i32,
            });
        } else {
            out.push(PlayerInput::UseItemOn {
                connection: c1,
                hand: Hand::Main,
                position: pos,
                face: 1,
                cursor: [0.5; 3],
                sequence: i as i32,
            });
        }
    }
    // Variant-specific late events.
    if i >= VARIANT_TICK {
        match (variant, i) {
            (Variant::A, 60) => out.push(cmd(1, "summon minecraft:creeper ~5 ~ ~")),
            (Variant::A, 100) => out.push(cmd(1, "summon minecraft:tnt ~3 ~ ~3")),
            (Variant::A, 80) => out.push(cmd(1, "fill ~4 ~ ~4 ~12 ~3 ~12 minecraft:stone")),
            (Variant::B, 80) => {
                out.push(cmd(1, "fill ~-12 ~ ~-12 ~-4 ~5 ~-4 minecraft:oak_planks"))
            }
            (Variant::A, 150) => out.push(cmd(1, "kill @e[type=minecraft:cow,limit=1]")),
            (Variant::B, 60) => out.push(cmd(1, "summon minecraft:spider ~-5 ~ ~")),
            (Variant::B, 100) => out.push(cmd(1, "setblock ~-2 ~6 ~-2 minecraft:gravel")),
            (Variant::B, 130) => out.push(cmd(1, "summon minecraft:bee ~ ~3 ~-4")),
            (Variant::B, 150) => out.push(cmd(1, "kill @e[type=minecraft:pig,limit=1]")),
            _ => {}
        }
    }
    out
}

// -------------------------------------------------------------------- hash

/// Everything that must replay identically, per tick.
#[derive(Default)]
struct Snap {
    resident: BTreeSet<(i32, i32)>,
    /// (chunk x, z, section index) -> palette + packed words hash.
    sections: BTreeMap<(i32, i32, usize), u64>,
    /// entity id -> body + cold row text (uuid and revision stamps included).
    entities: BTreeMap<i32, String>,
    /// entity id -> hash of pos, velocity, rotation.
    motion: BTreeMap<i32, u64>,
    players: BTreeMap<i32, u64>,
    advancements: BTreeMap<i32, u64>,
    nonplayer_entities: usize,
}

fn section_repr(section: &mc_world::ChunkSection) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    section.non_air_count().hash(&mut hasher);
    match (section.palette(), section.indices()) {
        (Some(palette), Some(indices)) => {
            1u8.hash(&mut hasher);
            for s in palette {
                s.0.hash(&mut hasher);
            }
            indices.bits_per_entry().hash(&mut hasher);
            indices.words().hash(&mut hasher);
        }
        _ => {
            0u8.hash(&mut hasher);
            section.get(0, 0, 0).0.hash(&mut hasher);
        }
    }
    hasher.finish()
}

fn snapshot(state: &TickState) -> Snap {
    let mut snap = Snap::default();
    for pos in state.world.resident_positions() {
        snap.resident.insert((pos.x, pos.z));
        let chunk = state.world.chunk(pos).expect("resident");
        for (index, section) in chunk.sections.iter().enumerate() {
            snap.sections
                .insert((pos.x, pos.z, index), section_repr(section));
        }
    }
    for index in 0..state.entities.len() as u32 {
        let slot = EntitySlot(index);
        let body = state.entities.body(slot);
        let id = body.id.0;
        snap.nonplayer_entities += usize::from(!matches!(
            body.kind,
            crate::entities::EntityKind::Player { .. }
        ));
        snap.entities
            .insert(id, format!("{body:?}|{:?}", state.entities.row(slot)));
        snap.motion.insert(
            id,
            h(&format!(
                "{:?}|{:?}|{:?}",
                state.entities.pos(slot),
                state.entities.vel(slot),
                state.entities.rot(slot)
            )),
        );
    }
    let mut ids: Vec<EntityId> = state.players.keys().copied().collect();
    ids.sort();
    for id in ids {
        let p = &state.players[&id];
        snap.players.insert(
            id.0,
            h(&format!(
                "{:?}|{:?}|{:?}|{:?}|{:?}|{:?}|{:?}",
                p.inventory,
                p.vitals,
                p.game_mode,
                p.active_effects,
                p.enchantment_seed,
                p.fall_distance,
                p.sprinting
            )),
        );
        snap.advancements
            .insert(id.0, h(&p.advancement_progress.to_string()));
    }
    snap
}

/// Names of the components that differ between two snapshots.
fn divergent_components(a: &Snap, b: &Snap) -> Vec<&'static str> {
    let mut out = Vec::new();
    if a.resident != b.resident {
        out.push("chunk-residency");
    }
    if a.sections != b.sections {
        out.push("blocks");
    }
    if a.entities.keys().ne(b.entities.keys()) {
        out.push("entity-set");
    }
    if a.entities != b.entities {
        out.push("entity-state");
    }
    if a.motion != b.motion {
        out.push("entity-motion");
    }
    if a.players != b.players {
        out.push("player-state");
    }
    if a.advancements != b.advancements {
        out.push("player-advancements");
    }
    out
}

// ---------------------------------------------------------------- drivers

/// One tick with chunk loads completed before it (loader timing is not part of
/// this canary). With `replay == None` the scripted client reads the server
/// state and generates inputs; with `Some(inputs)` the recorded inputs are
/// applied verbatim and the world is never consulted. Returns the inputs applied.
fn step(
    state: &mut TickState,
    i: u64,
    variant: Variant,
    replay: Option<&[PlayerInput]>,
) -> Vec<PlayerInput> {
    let want = state.desired_chunks();
    state.world.ensure_blocking(&want);
    let inputs = match replay {
        Some(recorded) => recorded.to_vec(),
        None => script(i, variant, state),
    };
    run_tick(state, &inputs);
    inputs
}

/// Record on world A (closed loop), replay open-loop on world B, compare every
/// tick. Returns `(tick, differing components)` for each divergent tick and
/// the final non-player entity count (non-vacuity).
fn compare_runs(variant: Variant, ticks: u64) -> (Vec<(u64, Vec<&'static str>)>, usize, usize) {
    let mut a = fresh_state();
    let mut b = fresh_state();
    let mut diverged = Vec::new();
    let mut ever = BTreeSet::new();
    for i in 0..ticks {
        let inputs = step(&mut a, i, variant, None);
        step(&mut b, i, variant, Some(&inputs));
        let (sa, sb) = (snapshot(&a), snapshot(&b));
        let comps = divergent_components(&sa, &sb);
        if !comps.is_empty() {
            if diverged.is_empty() {
                for (id, left) in &sa.entities {
                    if let Some(right) = sb.entities.get(id).filter(|right| *right != left) {
                        let at = left.bytes().zip(right.bytes()).position(|(x, y)| x != y);
                        let from = at.unwrap_or(0).saturating_sub(80);
                        eprintln!(
                            "first entity diff tick={i} id={id}\n A: ...{}\n B: ...{}",
                            &left[from..(from + 240).min(left.len())],
                            &right[from..(from + 240).min(right.len())]
                        );
                        break;
                    }
                }
            }
            diverged.push((i, comps));
        }
        ever.extend(sa.resident);
    }
    (diverged, snapshot(&a).nonplayer_entities, ever.len())
}

#[test]
fn recorded_inputs_replay_identically_with_blocking_residency() {
    for variant in [Variant::A, Variant::B] {
        let (diverged, entities, chunks) = compare_runs(variant, TOTAL_TICKS);
        assert!(
            diverged.is_empty(),
            "{variant:?}: {} divergent ticks, first {:?}",
            diverged.len(),
            diverged.first()
        );
        // Non-vacuous: the script spawned mobs/items and loaded terrain.
        assert!(entities >= 8, "{variant:?}: only {entities} entities");
        assert!(chunks >= 25, "{variant:?}: only {chunks} chunks");
    }
}
