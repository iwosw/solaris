//! Vanilla dimension identities and tick-owned, dimension-isolated worlds.
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Closed vanilla dimension registry. Unknown durable keys fail deserialization.
#[derive(
    Debug, Default, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
pub enum Dimension {
    #[default]
    #[serde(rename = "minecraft:overworld")]
    Overworld,
    #[serde(rename = "minecraft:the_nether")]
    Nether,
    #[serde(rename = "minecraft:the_end")]
    End,
}

impl Dimension {
    pub const ALL: [Self; 3] = [Self::Overworld, Self::Nether, Self::End];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Overworld => "minecraft:overworld",
            Self::Nether => "minecraft:the_nether",
            Self::End => "minecraft:the_end",
        }
    }

    pub fn parse(name: &str) -> Option<Self> {
        match name.strip_prefix("minecraft:").unwrap_or(name) {
            "overworld" => Some(Self::Overworld),
            "the_nether" => Some(Self::Nether),
            "the_end" => Some(Self::End),
            _ => None,
        }
    }

    pub fn directory(self, root: &Path) -> PathBuf {
        root.join("dimensions").join("minecraft").join(match self {
            Self::Overworld => "overworld",
            Self::Nether => "the_nether",
            Self::End => "the_end",
        })
    }

    pub fn geometry(self) -> mc_world::ChunkGeometry {
        match self {
            Self::Overworld => mc_world::OVERWORLD_GEOMETRY,
            Self::Nether => mc_worldgen::nether::nether_geometry(),
            Self::End => mc_worldgen::end::end_geometry(),
        }
    }

    pub const fn has_weather(self) -> bool {
        matches!(self, Self::Overworld)
    }

    pub const fn beds_work(self) -> bool {
        matches!(self, Self::Overworld)
    }

    pub const fn water_evaporates(self) -> bool {
        matches!(self, Self::Nether)
    }

    /// Vanilla `worldgen/noise_settings/{overworld,nether,end}.json`.
    pub const fn sea_level(self) -> i32 {
        match self {
            Self::Overworld => 63,
            Self::Nether => 32,
            Self::End => 0,
        }
    }

    pub const fn coordinate_scale(self) -> f64 {
        if matches!(self, Self::Nether) {
            8.0
        } else {
            1.0
        }
    }
}

/// Local records route by their envelope; player custody routes globally by UUID.
pub(crate) fn journal_for_dimension(
    batches: &[(u64, u64, Vec<crate::journal::JournalEntry>)],
    dimension: Dimension,
) -> Vec<(u64, u64, Vec<crate::journal::JournalEntry>)> {
    fn collect(
        entries: &[crate::journal::JournalEntry],
        owner: Dimension,
        requested: Dimension,
        out: &mut Vec<crate::journal::JournalEntry>,
    ) {
        for entry in entries {
            match entry {
                crate::journal::JournalEntry::Dimension { dimension, entries } => {
                    collect(entries, *dimension, requested, out)
                }
                crate::journal::JournalEntry::Player { .. }
                | crate::journal::JournalEntry::Statistics { .. } => out.push(entry.clone()),
                _ if owner == requested => out.push(entry.clone()),
                _ => {}
            }
        }
    }
    batches
        .iter()
        .map(|(lo, hi, entries)| {
            let mut out = Vec::new();
            collect(entries, Dimension::Overworld, dimension, &mut out);
            (*lo, *hi, out)
        })
        .collect()
}

/// Admit only globally unique native identities and reserve dragon multipart IDs.
pub(crate) fn validate_identity(
    state: &mut crate::tick::TickState,
) -> Result<(), crate::config::CoreError> {
    let mut ids = std::collections::HashSet::new();
    let mut uuids = std::collections::HashSet::new();
    let mut max_id = crate::end_fight::reserved_high_water(state)?;
    for world in
        std::iter::once(&*state).chain(state.other_dimensions.values().map(|world| &**world))
    {
        for slot in world.entities.slots() {
            let body = world.entities.body(slot);
            let row = world.entities.row(slot);
            if !ids.insert(body.id) || !uuids.insert(row.uuid) {
                return Err(crate::config::CoreError::Storage(format!(
                    "{}: duplicate entity identity wire-id {} UUID {}",
                    world.dimension.name(),
                    body.id.0,
                    row.uuid
                )));
            }
            max_id = max_id.max(body.id.0);
        }
    }
    state.entities.set_next_wire_id(max_id);
    for world in state.other_dimensions.values_mut() {
        world.entities.set_next_wire_id(max_id);
    }
    Ok(())
}

#[path = "dimensions_tick.rs"]
mod runtime;
pub(crate) use runtime::{run_tick, swap_globals};

#[path = "dimensions_checkpoint.rs"]
pub(crate) mod checkpoint;

#[path = "dimensions_travel.rs"]
pub(crate) mod travel;
pub use travel::locate_uuid;

#[path = "dimensions_rules.rs"]
pub(crate) mod rules;

#[path = "dimensions_recovery.rs"]
pub(crate) mod recovery;

#[path = "dimensions_globals.rs"]
pub(crate) mod globals;
pub use globals::RosterPlayer;

#[path = "dimensions_commands.rs"]
pub(crate) mod commands;

#[path = "dimensions_spawning.rs"]
pub(crate) mod spawning;

#[cfg(test)]
#[path = "dimensions_tests.rs"]
mod tests;
