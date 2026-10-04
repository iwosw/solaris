//! Prepared Earth-like roots and the native compact runtime source.

pub mod basin_fixture;
pub mod climate;
pub mod definition;
pub mod erosion;
pub mod grid;
pub mod hydrology;
pub mod materialization;
pub mod prepared;
pub mod region;
pub mod region_cache;
pub mod region_geology;
pub mod region_water;
pub mod runtime;
pub use runtime::{EarthBaseSource, EarthRuntime, EarthSourceCache};
pub mod tectonics;
pub mod world_plan;
