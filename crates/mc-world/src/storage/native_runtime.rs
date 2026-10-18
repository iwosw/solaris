//! Immutable natural geography prepared outside the simulation tick.

use crate::native::NativeError;

/// Immutable natural facts, prepared by an admitted caller outside the tick.
/// These are source geography, not effective blocks or gameplay policy.
#[derive(Debug, Clone, PartialEq)]
pub struct NaturalTerrainColumn {
    pub position: (i32, i32),
    pub ground_y: i32,
    pub biome: mc_data::Identifier,
    pub water_surface_y: Option<i32>,
    pub protected: bool,
    pub stable_ground: bool,
}

pub trait NativeNaturalSource: Send + Sync {
    fn seed(&self) -> i64;
    /// Pure procedural coverage check; it must not perform IO or preparation.
    fn contains_column(&self, x: i32, z: i32) -> bool;
    fn prepare_columns(
        &self,
        columns: &[(i32, i32)],
    ) -> Result<Vec<NaturalTerrainColumn>, NativeError>;
    fn prepare_village_sites(
        &self,
        min: (i32, i32),
        max: (i32, i32),
    ) -> Result<Vec<crate::GeneratedVillageSite>, NativeError>;
}
