//! Anti-xray engine mode 1 (Paper style): ores fully enclosed by opaque solid
//! blocks are sent to clients as the surrounding rock, so a modified client
//! cannot read them out of chunk data. Ores touching air, fluid or any other
//! non-opaque block stay as they are; mining next to a hidden ore reveals it
//! (see [`AntiXray::reveal_around`]).
//!
//! The hiding happens once per chunk at encode time inside the shared chunk
//! cache ([`crate::plan::ChunkPacketCache`]); the encoded frame is shared by
//! every viewer, so the cost does not scale with viewer count. The
//! authoritative chunk is never modified: sections holding hideable ores are
//! copied (palette storage is copy-on-write) and only the copies are encoded.
//! Because "enclosed" looks one block across chunk borders, the cache key of a
//! chunk includes its four side neighbours' revisions.

use mc_core::boundary::SnapshotChunk;
use mc_core::dimensions::Dimension;
use mc_data::Identifier;
use mc_data::block_light::BlockLightTable;
use mc_world::chunk::{BlockPos, Chunk};
use mc_world::{BlockRegistry, BlockStateId, ChunkSection, SECTION_DIM, SECTION_VOLUME};

use crate::config::AntiXraySection;
use crate::plan::find_chunk;

const OVERWORLD_ORES: [&str; 16] = [
    "coal_ore",
    "iron_ore",
    "copper_ore",
    "gold_ore",
    "redstone_ore",
    "lapis_ore",
    "diamond_ore",
    "emerald_ore",
    "deepslate_coal_ore",
    "deepslate_iron_ore",
    "deepslate_copper_ore",
    "deepslate_gold_ore",
    "deepslate_redstone_ore",
    "deepslate_lapis_ore",
    "deepslate_diamond_ore",
    "deepslate_emerald_ore",
];
const NETHER_ORES: [&str; 3] = ["nether_gold_ore", "nether_quartz_ore", "ancient_debris"];

/// One hideable ore state and the rock shown in its place.
#[derive(Debug, Clone, Copy)]
struct Ore {
    state: BlockStateId,
    /// Shown at `y >= 0`.
    upper: BlockStateId,
    /// Shown below `y = 0`: vanilla overworld rock is deepslate there, so a
    /// stone block inside it would itself mark a hidden ore.
    lower: BlockStateId,
}

/// Hideable ores of one dimension, sorted by state id.
#[derive(Debug)]
struct Rule {
    ores: Vec<Ore>,
}

impl Rule {
    fn ore(&self, state: BlockStateId) -> Option<&Ore> {
        self.ores
            .binary_search_by_key(&state, |ore| ore.state)
            .ok()
            .map(|i| &self.ores[i])
    }
}

/// Per-dimension anti-xray policy, built once at startup.
#[derive(Debug)]
pub struct AntiXray {
    /// By state id: full light-opaque suffocating cube, the blocks that hide
    /// what is behind them from a player.
    opaque: Vec<bool>,
    overworld: Option<Rule>,
    nether: Option<Rule>,
}

impl AntiXray {
    /// Build the policy; `None` when no dimension is enabled.
    pub fn new(
        blocks: &BlockRegistry,
        light: &BlockLightTable,
        config: &AntiXraySection,
    ) -> anyhow::Result<Option<Self>> {
        if !config.overworld && !config.nether {
            return Ok(None);
        }
        let opaque = (0..blocks.len() as u32)
            .map(|id| light.opacity(id) == Some(15) && light.suffocating(id) == Some(true))
            .collect();
        let overworld = config
            .overworld
            .then(|| overworld_rule(blocks))
            .transpose()?;
        let nether = config.nether.then(|| nether_rule(blocks)).transpose()?;
        Ok(Some(Self {
            opaque,
            overworld,
            nether,
        }))
    }

    fn rule(&self, dimension: Dimension) -> Option<&Rule> {
        match dimension {
            Dimension::Overworld => self.overworld.as_ref(),
            Dimension::Nether => self.nether.as_ref(),
            Dimension::End => None,
        }
    }

    /// Whether chunks of `dimension` are anti-xray processed.
    #[must_use]
    pub fn enabled(&self, dimension: Dimension) -> bool {
        self.rule(dimension).is_some()
    }

    /// Per-section replacements for `centre` (index = section index): copies
    /// with every enclosed hideable ore swapped for rock. Empty when nothing
    /// is hidden, so a chunk without enclosed ores encodes as stored.
    #[must_use]
    pub fn hide(
        &self,
        dimension: Dimension,
        around: &Surroundings<'_>,
    ) -> Vec<Option<ChunkSection>> {
        let Some(rule) = self.rule(dimension) else {
            return Vec::new();
        };
        let chunk = around.chunk;
        let min_y = chunk.geometry().min_y();
        let mut overrides: Vec<Option<ChunkSection>> = Vec::new();
        let mut cells: Vec<(u8, u8, u8, BlockStateId)> = Vec::new();
        for (index, section) in chunk.sections.iter().enumerate() {
            // A section that is one single state has no ore interior to hide.
            let (Some(palette), Some(indices)) = (section.palette(), section.indices()) else {
                continue;
            };
            let ore_of: Vec<Option<&Ore>> = palette.iter().map(|state| rule.ore(*state)).collect();
            if ore_of.iter().all(Option::is_none) {
                continue;
            }
            let base_y = min_y + (index * SECTION_DIM) as i32;
            cells.clear();
            for cell in 0..SECTION_VOLUME {
                let Some(ore) = ore_of[indices.get(cell) as usize] else {
                    continue;
                };
                let x = (cell % SECTION_DIM) as u8;
                let z = (cell / SECTION_DIM % SECTION_DIM) as u8;
                let sy = (cell / (SECTION_DIM * SECTION_DIM)) as u8;
                let y = base_y + i32::from(sy);
                if self.enclosed(around, i32::from(x), y, i32::from(z)) {
                    cells.push((x, sy, z, if y < 0 { ore.lower } else { ore.upper }));
                }
            }
            if cells.is_empty() {
                continue;
            }
            overrides.resize_with(index + 1, || None);
            overrides[index] = Some(section.with_cells_replaced(&cells));
        }
        overrides
    }

    /// Positions next to a changed block `position` whose hideable ore is not
    /// enclosed any more in `chunks`: clients that were sent the rock stand-in
    /// need the real state now. Ores that are still enclosed are not listed.
    pub fn reveal_around(
        &self,
        dimension: Dimension,
        chunks: &[SnapshotChunk],
        position: BlockPos,
        revealed: &mut Vec<BlockPos>,
    ) {
        let Some(rule) = self.rule(dimension) else {
            return;
        };
        for (dx, dy, dz) in [
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let neighbour = BlockPos {
                x: position.x + dx,
                y: position.y + dy,
                z: position.z + dz,
            };
            let Some(resident) = find_chunk(
                chunks,
                (neighbour.x.div_euclid(16), neighbour.z.div_euclid(16)),
            ) else {
                continue;
            };
            let (lx, lz) = (neighbour.x.rem_euclid(16), neighbour.z.rem_euclid(16));
            let Some(state) = resident.chunk.get_block(lx as u8, neighbour.y, lz as u8) else {
                continue;
            };
            if rule.ore(state).is_some()
                && !self.enclosed(&Surroundings::of(chunks, resident), lx, neighbour.y, lz)
            {
                revealed.push(neighbour);
            }
        }
    }

    /// All six face neighbours opaque. Outside the build height counts as
    /// opaque (bedrock/void), an unloaded side chunk as open so no ore is
    /// hidden on guesswork.
    fn enclosed(&self, around: &Surroundings<'_>, x: i32, y: i32, z: i32) -> bool {
        [
            (x - 1, y, z),
            (x + 1, y, z),
            (x, y - 1, z),
            (x, y + 1, z),
            (x, y, z - 1),
            (x, y, z + 1),
        ]
        .into_iter()
        .all(|(x, y, z)| {
            around
                .state(x, y, z)
                .is_none_or(|state| self.opaque.get(state.0 as usize).copied().unwrap_or(false))
        })
    }
}

/// A chunk and its four side neighbours (`-x`, `+x`, `-z`, `+z`), as resident
/// in one snapshot.
pub struct Surroundings<'a> {
    chunk: &'a Chunk,
    sides: [Option<&'a Chunk>; 4],
}

impl<'a> Surroundings<'a> {
    /// Look the side neighbours of `centre` up in the sorted snapshot chunks.
    #[must_use]
    pub fn of(chunks: &'a [SnapshotChunk], centre: &'a SnapshotChunk) -> Self {
        let (x, z) = (centre.pos.x, centre.pos.z);
        let side = |x, z| find_chunk(chunks, (x, z)).map(|resident| &*resident.chunk);
        Self {
            chunk: &centre.chunk,
            sides: [
                side(x - 1, z),
                side(x + 1, z),
                side(x, z - 1),
                side(x, z + 1),
            ],
        }
    }

    /// State at chunk-local `x`/`z` in `-1..=16`. `None`: outside the build
    /// height. An unloaded side chunk yields the never-opaque [`OPEN`].
    fn state(&self, x: i32, y: i32, z: i32) -> Option<BlockStateId> {
        let (chunk, lx, lz) = if x < 0 {
            (self.sides[0], x + 16, z)
        } else if x >= 16 {
            (self.sides[1], x - 16, z)
        } else if z < 0 {
            (self.sides[2], x, z + 16)
        } else if z >= 16 {
            (self.sides[3], x, z - 16)
        } else {
            (Some(self.chunk), x, z)
        };
        match chunk {
            // Unloaded neighbour: report a state that is never opaque.
            None => Some(OPEN),
            Some(chunk) => chunk.get_block(lx as u8, y, lz as u8),
        }
    }
}

/// Sentinel outside the registry: never opaque.
const OPEN: BlockStateId = BlockStateId(u32::MAX);

fn overworld_rule(blocks: &BlockRegistry) -> anyhow::Result<Rule> {
    let stone = default_state(blocks, "stone")?;
    let deepslate = default_state(blocks, "deepslate")?;
    let mut ores = Vec::new();
    for name in OVERWORLD_ORES {
        let (upper, lower) = if name.starts_with("deepslate_") {
            (deepslate, deepslate)
        } else {
            (stone, deepslate)
        };
        push_ore_states(blocks, name, upper, lower, &mut ores)?;
    }
    ores.sort_by_key(|ore| ore.state);
    Ok(Rule { ores })
}

fn nether_rule(blocks: &BlockRegistry) -> anyhow::Result<Rule> {
    let netherrack = default_state(blocks, "netherrack")?;
    let mut ores = Vec::new();
    for name in NETHER_ORES {
        push_ore_states(blocks, name, netherrack, netherrack, &mut ores)?;
    }
    ores.sort_by_key(|ore| ore.state);
    Ok(Rule { ores })
}

fn default_state(blocks: &BlockRegistry, name: &str) -> anyhow::Result<BlockStateId> {
    let id = Identifier::parse(format!("minecraft:{name}"))?;
    blocks
        .block(&id)
        .map(|block| block.default)
        .ok_or_else(|| anyhow::anyhow!("anti-xray: block {id} missing from the block registry"))
}

fn push_ore_states(
    blocks: &BlockRegistry,
    name: &str,
    upper: BlockStateId,
    lower: BlockStateId,
    ores: &mut Vec<Ore>,
) -> anyhow::Result<()> {
    let id = Identifier::parse(format!("minecraft:{name}"))?;
    let block = blocks
        .block(&id)
        .ok_or_else(|| anyhow::anyhow!("anti-xray: block {id} missing from the block registry"))?;
    ores.extend(block.states.iter().map(|&state| Ore {
        state,
        upper,
        lower,
    }));
    Ok(())
}

#[cfg(test)]
#[path = "anti_xray_tests.rs"]
mod anti_xray_tests;
