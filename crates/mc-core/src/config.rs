//! [`Core`] configuration and error type.

use std::path::PathBuf;

use thiserror::Error;

/// Configuration for [`crate::Core`].
#[derive(Debug, Clone)]
pub struct CoreConfig {
    /// World root: `dimensions/minecraft/*`, `players/data/`, global `data/` and private `journal/`.
    pub world_dir: PathBuf,
    /// World seed for chunk generation.
    pub seed: i64,
    /// Chunk view distance (radius) sent to each player.
    pub view_distance: u8,
    /// Chunk gameplay simulation radius (independent of client residency).
    pub simulation_distance: u8,
    /// Bounded submit queue depth for [`PlayerInput`](crate::boundary::PlayerInput).
    pub input_queue_cap: usize,
    /// Broadcast channel depth for [`TickOutput`](crate::boundary::TickOutput).
    pub broadcast_cap: usize,
    /// Per-connection reliable [`PlayerChange`](crate::boundary::PlayerChange)
    /// outbox depth. Overflow disconnects the connection.
    pub player_outbox_cap: usize,
    /// Journal fsync cadence: fsync after this many appended batches.
    pub journal_fsync_every_batches: u32,
    /// Ticks of journal retained past the last complete checkpoint.
    pub checkpoint_every_ticks: u64,
    /// Vanilla content-cache root (`data/minecraft/...`, `reports/...`).
    /// Required outside tests; missing content is a startup error. Only
    /// `#[cfg(test)]` builds fall back to the checkout's `data/vanilla`.
    pub vanilla_data_dir: Option<PathBuf>,
    /// Strictly discovered production component package directory supplied by edge.
    pub plugin_directory: Option<PathBuf>,
}

impl CoreConfig {
    /// Minimal config; queue depths and cadences take tested defaults.
    #[must_use]
    pub fn new(world_dir: PathBuf, seed: i64) -> Self {
        Self {
            world_dir,
            seed,
            view_distance: 8,
            simulation_distance: 6,
            input_queue_cap: 4096,
            broadcast_cap: 32,
            player_outbox_cap: 512,
            journal_fsync_every_batches: 10,
            checkpoint_every_ticks: 6000,
            vanilla_data_dir: None,
            plugin_directory: None,
        }
    }
}

/// Every failure mode of [`crate::Core`] startup and operation.
#[derive(Debug, Error)]
#[cfg_attr(test, derive(Clone))]
pub enum CoreError {
    /// Component discovery, configuration or runtime startup failed closed.
    #[error("plugin: {0}")]
    Plugin(String),
    /// World directory or journal setup failed.
    #[error("storage: {0}")]
    Storage(String),
    /// Block/entity/item registries did not build.
    #[error("registry: {0}")]
    Registry(String),
    /// Worldgen router or density data failed to load.
    #[error("worldgen: {0}")]
    Worldgen(String),
    /// Journal recovery found no complete batch and no manifest.
    #[error("recovery: {0}")]
    Recovery(String),
    /// The tick thread has stopped.
    #[error("tick thread stopped")]
    TickStopped,
    /// Remote ticket exceeds its budget.
    #[error("ticket over budget: {0}")]
    TicketOverBudget(String),
}
