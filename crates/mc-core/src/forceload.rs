//! Vanilla `/forceload add|remove|query` and the 26.1.2 chunk-ticket extents.
//!
//! Sources: `ForceLoadCommand` (256-chunk cap per command, column-pos syntax,
//! feedback keys), `TicketType.FORCED` (persisted, loading + simulation, no
//! timeout), `ChunkMap.FORCED_TICKET_LEVEL` = entity ticking (31) and
//! `TicketStorage.addTicketWithRadius` (ticket level 33 - radius).
//!
//! A forced chunk is the persisted native ticket `minecraft:forced:<x>:<z>`
//! with radius 0 in the owning dimension's [`TickState::tickets`]. Checkpoints,
//! the tick WAL and the Anvil `chunk_tickets.dat` conversion already carry it.
//! Level 31 ticks entities only in the centre, blocks through distance one,
//! and loads chunks through distance two.

use crate::boundary::ChunkPos;
use crate::commands::{CommandError, Coord, coord};
use crate::tick::TickState;

/// Owner prefix of persisted forced chunks, shared with `chunk_tickets.dat`.
pub(crate) const OWNER_PREFIX: &str = "minecraft:forced:";
/// Vanilla `ForceLoadCommand.MAX_CHUNK_LIMIT`.
const MAX_CHUNKS: i64 = 256;
/// Vanilla rejects columns outside +-30,000,000 blocks.
const WORLD_LIMIT: i32 = 30_000_000;
/// A level-31 ticket keeps this many chunks around its centre loaded
/// (`FULL` level 33 is reached at distance 2).
pub(crate) const LOAD_RADIUS: i32 = 2;
/// Level 32 ticks blocks but not entities.
pub(crate) const BLOCK_RADIUS: i32 = 1;
/// Only level 31 ticks entities.
pub(crate) const ENTITY_RADIUS: i32 = 0;

const USAGE: &str =
    "forceload (add|remove) <from> [<to>] | forceload remove all | forceload query [<pos>]";

/// Parsed `/forceload`; positions are `~`-relative column coordinates.
#[derive(Debug, Clone, PartialEq)]
pub enum ForceLoadCommand {
    Add {
        from: [Coord; 2],
        to: Option<[Coord; 2]>,
    },
    Remove {
        from: [Coord; 2],
        to: Option<[Coord; 2]>,
    },
    RemoveAll,
    Query {
        pos: Option<[Coord; 2]>,
    },
}

/// One translatable reply; failures render red like a vanilla command error.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Reply {
    pub key: &'static str,
    pub args: Vec<String>,
    pub failure: bool,
}

fn success(key: &'static str, args: Vec<String>) -> Reply {
    Reply {
        key,
        args,
        failure: false,
    }
}

fn failure(key: &'static str, args: Vec<String>) -> Reply {
    Reply {
        key,
        args,
        failure: true,
    }
}

pub(crate) fn owner(chunk: ChunkPos) -> String {
    format!("{OWNER_PREFIX}{}:{}", chunk.x, chunk.z)
}

pub(crate) fn is_forced_owner(owner: &str) -> bool {
    owner.starts_with(OWNER_PREFIX)
}

fn column(x: &str, z: &str) -> Option<[Coord; 2]> {
    let columns = [coord(x)?, coord(z)?];
    // ColumnPosArgument accepts world (`~`) coordinates only, never local `^`,
    // and absolute columns are integers.
    columns
        .iter()
        .all(|c| !c.local && (c.relative || c.value.fract() == 0.0))
        .then_some(columns)
}

pub fn parse(args: &[&str]) -> Result<ForceLoadCommand, CommandError> {
    let usage = || CommandError::Usage(USAGE);
    let area = |words: &[&str]| -> Result<([Coord; 2], Option<[Coord; 2]>), CommandError> {
        match words {
            [x, z] => Ok((column(x, z).ok_or_else(usage)?, None)),
            [x1, z1, x2, z2] => Ok((
                column(x1, z1).ok_or_else(usage)?,
                Some(column(x2, z2).ok_or_else(usage)?),
            )),
            _ => Err(usage()),
        }
    };
    match args {
        ["add", rest @ ..] => {
            let (from, to) = area(rest)?;
            Ok(ForceLoadCommand::Add { from, to })
        }
        ["remove", "all"] => Ok(ForceLoadCommand::RemoveAll),
        ["remove", rest @ ..] => {
            let (from, to) = area(rest)?;
            Ok(ForceLoadCommand::Remove { from, to })
        }
        ["query"] => Ok(ForceLoadCommand::Query { pos: None }),
        ["query", x, z] => Ok(ForceLoadCommand::Query {
            pos: Some(column(x, z).ok_or_else(usage)?),
        }),
        _ => Err(usage()),
    }
}

/// Forced chunks of this dimension in vanilla's list order: packed
/// `x & 0xFFFFFFFF | z << 32` ascending, i.e. by `z` then unsigned `x`.
pub(crate) fn forced_chunks(state: &TickState) -> Vec<ChunkPos> {
    let mut chunks: Vec<ChunkPos> = state
        .tickets
        .range(OWNER_PREFIX.to_owned()..)
        .take_while(|(owner, _)| is_forced_owner(owner))
        .map(|(_, (chunk, _))| *chunk)
        .collect();
    chunks.sort_unstable_by_key(|chunk| (chunk.z, chunk.x as u32));
    chunks
}

fn label(chunk: ChunkPos) -> String {
    format!("[{}, {}]", chunk.x, chunk.z)
}

fn column_block(
    state: &TickState,
    executor: crate::boundary::EntityId,
    column: [Coord; 2],
) -> Result<[i32; 2], Reply> {
    let (base, _) = crate::command_console::pose(state, executor)
        .ok_or_else(|| failure("argument.entity.notfound.entity", Vec::new()))?;
    let x = column[0].resolve(base[0]).floor();
    let z = column[1].resolve(base[2]).floor();
    if !x.is_finite()
        || !z.is_finite()
        || x.abs() > f64::from(WORLD_LIMIT)
        || z.abs() > f64::from(WORLD_LIMIT)
    {
        return Err(failure("argument.pos.outofworld", Vec::new()));
    }
    Ok([x as i32, z as i32])
}

pub(crate) fn execute(
    state: &mut TickState,
    executor: crate::boundary::EntityId,
    command: &ForceLoadCommand,
) -> Reply {
    run(state, executor, command).unwrap_or_else(|reply| reply)
}

fn run(
    state: &mut TickState,
    executor: crate::boundary::EntityId,
    command: &ForceLoadCommand,
) -> Result<Reply, Reply> {
    let dimension = state.dimension.name().to_owned();
    match command {
        ForceLoadCommand::Add { from, to } | ForceLoadCommand::Remove { from, to } => {
            let add = matches!(command, ForceLoadCommand::Add { .. });
            let from = column_block(state, executor, *from)?;
            let to = match to {
                Some(to) => column_block(state, executor, *to)?,
                None => from,
            };
            let (min_x, max_x) = (from[0].min(to[0]), from[0].max(to[0]));
            let (min_z, max_z) = (from[1].min(to[1]), from[1].max(to[1]));
            // `x < 30000000` is the vanilla upper bound (exclusive).
            if min_x < -WORLD_LIMIT
                || min_z < -WORLD_LIMIT
                || max_x >= WORLD_LIMIT
                || max_z >= WORLD_LIMIT
            {
                return Err(failure("argument.pos.outofworld", Vec::new()));
            }
            let (min_cx, max_cx) = (min_x.div_euclid(16), max_x.div_euclid(16));
            let (min_cz, max_cz) = (min_z.div_euclid(16), max_z.div_euclid(16));
            let count = i64::from(max_cx - min_cx + 1) * i64::from(max_cz - min_cz + 1);
            if count > MAX_CHUNKS {
                return Err(failure(
                    "commands.forceload.toobig",
                    vec![MAX_CHUNKS.to_string(), count.to_string()],
                ));
            }
            let mut changed = 0_usize;
            let mut first = None;
            for x in min_cx..=max_cx {
                for z in min_cz..=max_cz {
                    let chunk = ChunkPos { x, z };
                    let was_changed = if add {
                        state.tickets.insert(owner(chunk), (chunk, 0)).is_none()
                    } else {
                        state.tickets.remove(&owner(chunk)).is_some()
                    };
                    if was_changed {
                        changed += 1;
                        first.get_or_insert(chunk);
                    }
                }
            }
            let Some(first) = first else {
                return Err(failure(
                    if add {
                        "commands.forceload.added.failure"
                    } else {
                        "commands.forceload.removed.failure"
                    },
                    Vec::new(),
                ));
            };
            Ok(if changed == 1 {
                success(
                    if add {
                        "commands.forceload.added.single"
                    } else {
                        "commands.forceload.removed.single"
                    },
                    vec![label(first), dimension],
                )
            } else {
                success(
                    if add {
                        "commands.forceload.added.multiple"
                    } else {
                        "commands.forceload.removed.multiple"
                    },
                    vec![
                        changed.to_string(),
                        dimension,
                        label(ChunkPos {
                            x: min_cx,
                            z: min_cz,
                        }),
                        label(ChunkPos {
                            x: max_cx,
                            z: max_cz,
                        }),
                    ],
                )
            })
        }
        ForceLoadCommand::RemoveAll => {
            for chunk in forced_chunks(state) {
                state.tickets.remove(&owner(chunk));
            }
            Ok(success("commands.forceload.removed.all", vec![dimension]))
        }
        ForceLoadCommand::Query { pos: None } => {
            let chunks = forced_chunks(state);
            let list = chunks
                .iter()
                .map(|chunk| label(*chunk))
                .collect::<Vec<_>>()
                .join(", ");
            Ok(match chunks.len() {
                0 => failure("commands.forceload.added.none", vec![dimension]),
                1 => success("commands.forceload.list.single", vec![dimension, list]),
                count => success(
                    "commands.forceload.list.multiple",
                    vec![count.to_string(), dimension, list],
                ),
            })
        }
        ForceLoadCommand::Query { pos: Some(pos) } => {
            let [x, z] = column_block(state, executor, *pos)?;
            let chunk = ChunkPos {
                x: x.div_euclid(16),
                z: z.div_euclid(16),
            };
            if state.tickets.contains_key(&owner(chunk)) {
                Ok(success(
                    "commands.forceload.query.success",
                    vec![label(chunk), dimension],
                ))
            } else {
                Err(failure(
                    "commands.forceload.query.failure",
                    vec![label(chunk), dimension],
                ))
            }
        }
    }
}

#[cfg(test)]
#[path = "forceload_tests.rs"]
mod tests;
