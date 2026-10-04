//! Validated immutable generation inputs; operating settings never enter identity.
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::LazyLock;

use super::prepared::PrepareError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlaneBounds {
    pub min_x: i64,
    pub max_x_exclusive: i64,
    pub min_z: i64,
    pub max_z_exclusive: i64,
}

impl PlaneBounds {
    pub fn validate(self, chunk_aligned: bool) -> Result<(), PrepareError> {
        let width = self.max_x_exclusive.checked_sub(self.min_x);
        let depth = self.max_z_exclusive.checked_sub(self.min_z);
        if width.is_none_or(|n| n <= 0) || depth.is_none_or(|n| n <= 0) {
            return Err(PrepareError::Invalid(
                "domain must have ordered non-empty extents",
            ));
        }
        width
            .unwrap()
            .checked_mul(depth.unwrap())
            .ok_or(PrepareError::Invalid("block area overflow"))?;
        for edge in [
            self.min_x,
            self.max_x_exclusive,
            self.min_z,
            self.max_z_exclusive,
        ] {
            i32::try_from(edge).map_err(|_| PrepareError::Invalid("coordinate exceeds i32"))?;
            if !(-30_000_000..=30_000_000).contains(&edge) {
                return Err(PrepareError::Invalid(
                    "coordinate exceeds supported compact client/physics domain",
                ));
            }
            if chunk_aligned && edge.rem_euclid(16) != 0 {
                return Err(PrepareError::Invalid(
                    "playable edges must be chunk-aligned (16 blocks)",
                ));
            }
        }
        Ok(())
    }

    pub fn contains(self, other: Self) -> bool {
        self.min_x <= other.min_x
            && self.max_x_exclusive >= other.max_x_exclusive
            && self.min_z <= other.min_z
            && self.max_z_exclusive >= other.max_z_exclusive
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DefinitionInput {
    pub topology: String,
    pub domain: PlaneBounds,
    pub horizontal_meters_per_block: f64,
    pub macro_cell_m: f64,
    pub climate_cell_m: f64,
    pub refinement_profile: String,
    pub vertical_profile: String,
    pub seed: i64,
    pub content_fingerprint: String,
    pub math_fingerprint: String,
    pub equator_z_m: f64,
    pub climate_radius_m: f64,
}

/// Construct through `validate`; no mutable access or unchecked deserialization.
#[derive(Debug, Clone, PartialEq)]
pub struct WorldDefinition(DefinitionInput);

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct WorldEstimate {
    pub area_m2: f64,
    pub root_columns: u64,
    pub root_rows: u64,
    pub root_cells: u64,
    pub climate_cells: u64,
    pub root_partial_edge_x_m: f64,
    pub root_partial_edge_z_m: f64,
    pub climate_partial_edge_x_m: f64,
    pub climate_partial_edge_z_m: f64,
    /// Conservative encoded-root bound, including catalogs, checked before decode.
    pub root_payload_limit_bytes: u64,
    /// Conservative in-memory builder/decode bound; no precision degradation.
    pub builder_peak_bytes: u64,
}

impl DefinitionInput {
    /// Explicit finite known basin, retained independently of the plane solver.
    pub fn basin_fixture(seed: i64) -> Self {
        static CONTENT: LazyLock<String> = LazyLock::new(|| {
            hex_hash(include_bytes!("../../../mc-data/data/required_blocks.json"))
        });
        Self {
            topology: "plane".into(),
            domain: PlaneBounds {
                min_x: 0,
                max_x_exclusive: 80,
                min_z: 0,
                max_z_exclusive: 80,
            },
            horizontal_meters_per_block: 1.0,
            macro_cell_m: 16.0,
            climate_cell_m: 16.0,
            refinement_profile: "w0-basin".into(),
            vertical_profile: "compact".into(),
            seed,
            content_fingerprint: CONTENT.clone(),
            math_fingerprint: "w1-integer-basin-compact-y-v1".into(),
            equator_z_m: 0.0,
            climate_radius_m: 6_371_000.0,
        }
    }

    /// Small physical reference profile; operators may change immutable extents/precision.
    pub fn earth_root(seed: i64) -> Self {
        Self {
            domain: PlaneBounds {
                min_x: -8192,
                max_x_exclusive: 8192,
                min_z: -8192,
                max_z_exclusive: 8192,
            },
            macro_cell_m: 256.0,
            climate_cell_m: 1024.0,
            refinement_profile: "earth-root-v1".into(),
            math_fingerprint: "w2-f64-canonical-order-v1".into(),
            ..Self::basin_fixture(seed)
        }
    }

    pub fn validate(mut self) -> Result<WorldDefinition, PrepareError> {
        if self.topology != "plane" {
            return Err(PrepareError::Invalid(
                "only finite plane topology is supported",
            ));
        }
        if self.vertical_profile != "compact" {
            return Err(PrepareError::Invalid(
                "metric height is unsupported until the height-capable runtime",
            ));
        }
        self.domain.validate(false)?;
        for scale in [
            self.horizontal_meters_per_block,
            self.macro_cell_m,
            self.climate_cell_m,
            self.climate_radius_m,
        ] {
            if !scale.is_finite() || scale <= 0.0 {
                return Err(PrepareError::Invalid("scales must be finite and positive"));
            }
        }
        if !self.equator_z_m.is_finite() {
            return Err(PrepareError::Invalid("equator datum must be finite"));
        }
        // Normalize negative zero, without changing physical scale or precision.
        if self.equator_z_m == 0.0 {
            self.equator_z_m = 0.0;
        }
        for id in [
            &self.refinement_profile,
            &self.content_fingerprint,
            &self.math_fingerprint,
        ] {
            if id.is_empty() || id.len() > 256 || id.trim() != id {
                return Err(PrepareError::Invalid(
                    "profile/fingerprint must be non-empty, bounded and normalized",
                ));
            }
        }
        let definition = WorldDefinition(self);
        definition.estimate()?;
        Ok(definition)
    }
}

impl WorldDefinition {
    pub fn input(&self) -> &DefinitionInput {
        &self.0
    }

    pub fn canonical_bytes(&self) -> Result<Vec<u8>, PrepareError> {
        Ok(toml::to_string(&self.0)?.into_bytes())
    }

    pub fn estimate(&self) -> Result<WorldEstimate, PrepareError> {
        let width = (self.0.domain.max_x_exclusive - self.0.domain.min_x) as f64
            * self.0.horizontal_meters_per_block;
        let depth = (self.0.domain.max_z_exclusive - self.0.domain.min_z) as f64
            * self.0.horizontal_meters_per_block;
        let area_m2 = width * depth;
        if !width.is_finite() || !depth.is_finite() || !area_m2.is_finite() || area_m2 <= 0.0 {
            return Err(PrepareError::Invalid("physical area overflow/underflow"));
        }
        let grid = |step: f64| -> Result<(u64, u64, u64), PrepareError> {
            let x = (width / step).ceil();
            let z = (depth / step).ceil();
            if !x.is_finite()
                || !z.is_finite()
                || x < 1.0
                || z < 1.0
                || x >= u64::MAX as f64
                || z >= u64::MAX as f64
            {
                return Err(PrepareError::Invalid("grid axis overflow/underflow"));
            }
            let (x, z) = (x as u64, z as u64);
            let cells = x
                .checked_mul(z)
                .ok_or(PrepareError::Invalid("grid cell count overflow"))?;
            Ok((x, z, cells))
        };
        let (root_columns, root_rows, root_cells) = grid(self.0.macro_cell_m)?;
        let (_, _, climate_cells) = grid(self.0.climate_cell_m)?;
        let root_payload_limit_bytes = root_cells
            .checked_add(climate_cells)
            .and_then(|n| n.checked_mul(8192))
            .and_then(|n| n.checked_add(1024 * 1024))
            .ok_or(PrepareError::Invalid("root byte bound overflow"))?;
        let builder_peak_bytes = root_payload_limit_bytes
            .checked_mul(4)
            .ok_or(PrepareError::Invalid("builder memory bound overflow"))?;
        Ok(WorldEstimate {
            area_m2,
            root_columns,
            root_rows,
            root_cells,
            climate_cells,
            root_partial_edge_x_m: width.rem_euclid(self.0.macro_cell_m),
            root_partial_edge_z_m: depth.rem_euclid(self.0.macro_cell_m),
            climate_partial_edge_x_m: width.rem_euclid(self.0.climate_cell_m),
            climate_partial_edge_z_m: depth.rem_euclid(self.0.climate_cell_m),
            root_payload_limit_bytes,
            builder_peak_bytes,
        })
    }

    pub fn check_preparation_support(&self) -> Result<(), PrepareError> {
        if self.0.refinement_profile == "w0-basin" {
            let expected = DefinitionInput::basin_fixture(self.0.seed).validate()?;
            if self != &expected {
                return Err(PrepareError::Invalid(
                    "w0-basin requires its exact explicit inputs",
                ));
            }
        } else if self.0.refinement_profile != "earth-root-v1"
            || self.0.math_fingerprint != "w2-f64-canonical-order-v1"
            || self.0.content_fingerprint
                != DefinitionInput::basin_fixture(self.0.seed).content_fingerprint
        {
            return Err(PrepareError::Invalid(
                "unsupported root/refinement or pinned math profile",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PlayableBounds {
    pub bounds: PlaneBounds,
    pub operating_revision: u64,
}

impl PlayableBounds {
    pub fn validate(&self, definition: &WorldDefinition) -> Result<(), PrepareError> {
        self.bounds.validate(true)?;
        if self.operating_revision == 0 || !definition.input().domain.contains(self.bounds) {
            return Err(PrepareError::Invalid(
                "playable bounds must be contained in the domain with a nonzero revision",
            ));
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorldIdentity([u8; 32]);

impl WorldIdentity {
    pub fn derive(
        definition: &WorldDefinition,
        algorithm_artifact: [u8; 32],
        root_hash: [u8; 32],
    ) -> Result<Self, PrepareError> {
        let definition_bytes = definition.canonical_bytes()?;
        let mut hash = Sha256::new();
        hash.update(b"Solaris-Earth-WorldIdentity-W1\0");
        hash.update((definition_bytes.len() as u64).to_be_bytes());
        hash.update(definition_bytes);
        hash.update(algorithm_artifact);
        hash.update(root_hash);
        Ok(Self(hash.finalize().into()))
    }
    pub fn bytes(self) -> [u8; 32] {
        self.0
    }
    pub fn hex(self) -> String {
        hex_bytes(&self.0)
    }
}

pub(super) fn hash(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}
fn hex_hash(bytes: &[u8]) -> String {
    hex_bytes(&hash(bytes))
}
fn hex_bytes(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(text, "{byte:02x}").expect("String write cannot fail");
    }
    text
}

/// Pins every executable root, refinement and base-materialization source.
pub fn algorithm_artifact_id() -> [u8; 32] {
    static ARTIFACT: LazyLock<[u8; 32]> = LazyLock::new(|| {
        let mut hash = Sha256::new();
        hash.update(b"Solaris-W5-earth-root-region-base-artifact-v1\0");
        hash.update(include_bytes!("basin_fixture.rs"));
        hash.update(include_bytes!("definition.rs"));
        hash.update(include_bytes!("grid.rs"));
        hash.update(include_bytes!("tectonics.rs"));
        hash.update(include_bytes!("climate.rs"));
        hash.update(include_bytes!("hydrology.rs"));
        hash.update(include_bytes!("erosion.rs"));
        hash.update(include_bytes!("world_plan.rs"));
        hash.update(include_bytes!("region.rs"));
        hash.update(include_bytes!("region_water.rs"));
        hash.update(include_bytes!("region_geology.rs"));
        hash.update(include_bytes!("materialization.rs"));
        hash.update(include_bytes!("runtime.rs"));
        hash.update(include_bytes!("../village/plan_source.rs"));
        hash.update(include_bytes!("../village/piece.rs"));
        hash.update(include_bytes!("../village/beard.rs"));
        hash.update(include_bytes!("../village/solver.rs"));
        hash.update(include_bytes!("../village/closure.rs"));
        hash.update(include_bytes!("../village/decor.rs"));
        hash.update(include_bytes!("../village/placement.rs"));
        hash.update(include_bytes!("../village/processors.rs"));
        hash.update(include_bytes!("../village/shapes.rs"));
        hash.update(include_bytes!("../vanilla_features/mod.rs"));
        hash.update(include_bytes!("../vanilla_features/random.rs"));
        hash.update(include_bytes!("../vanilla_features/synth.rs"));
        hash.update(include_bytes!("../vanilla_features/placement.rs"));
        hash.update(include_bytes!("../vanilla_features/provider.rs"));
        hash.update(include_bytes!("../vanilla_features/tree.rs"));
        hash.update(include_bytes!("../vanilla_features/execute.rs"));
        hash.update(include_bytes!("../structures.rs"));
        hash.update(include_bytes!("../noise.rs"));
        hash.finalize().into()
    });
    *ARTIFACT
}
