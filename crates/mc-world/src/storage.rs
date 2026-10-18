//! World-root admission and immutable natural geography shared by native sources.

use std::path::PathBuf;

use thiserror::Error;

mod native_runtime;
mod world_lease;

pub use native_runtime::{NativeNaturalSource, NaturalTerrainColumn};
pub use world_lease::{WorldRootLease, acquire_exclusive_world_root_lease};

#[derive(Debug, Error)]
pub enum WorldError {
    #[error("writable world root is already leased: {root}; holder metadata: {metadata}")]
    WorldLocked { root: PathBuf, metadata: String },
    #[error("world lease {operation} failed at {path}: {source}")]
    WorldLeaseIo {
        operation: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
