//! Per-block-state light metadata. When a vanilla sidecar is configured,
//! Solaris loads `data/vanilla/reports/block_light.json` as a required
//! runtime oracle. Without a sidecar, local fallback mode builds a
//! conservative table from the embedded blocks report.
//!
//! - `emission` — `BlockState.getLightEmission()`, 0..=15 (luminance
//!   the block radiates; torches=14, glowstone=15, soul_lantern=10).
//! - `opacity` — `BlockState.getLightDampening()`, 0..=15 (loss per
//!   step when light passes through this cell; vanilla mostly uses 0
//!   for fully transparent, 15 for fully opaque, and 1 for "soft"
//!   attenuators like water/ice/leaves).
//! - `propagates_sky` — `BlockState.propagatesSkylightDown()`, the
//!   predicate that drives sky-light's heightmap shortcut.
//! - `suffocating` — `BlockState.isSuffocating()`, used by gameplay
//!   checks that require a full blocking collision shape.
//!
//! The script's output is data (ADR 0001); the table is loaded at
//! server start with the same posture as `blocks.json`.
//!
//! See `tools/extract-block-light/LightExtractor.java` for the
//! extraction. The Rust side is intentionally schema-stable: any
//! incompatible change ships under a bumped `version` field, the
//! script regenerates the file, and the loader's mismatch error is
//! surfaced as a clear runtime message.

use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::{blocks::BlockReport, read_json_file};

#[derive(Debug, Error)]
pub enum BlockLightError {
    #[error("block_light.json not found at {0}; did you run tools/extract-block-light.sh?")]
    Missing(PathBuf),
    #[error("io error reading {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("parse error in {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: serde_json::Error,
    },
    #[error(
        "entries length {entries} does not match max_state_id {max_state_id} (expected {expected})"
    )]
    LengthMismatch {
        entries: usize,
        max_state_id: u32,
        expected: usize,
    },
    #[error(
        "entry at state-id {state_id} has {got} elements; \
         expected 4 ([emission, opacity, propagates_sky, suffocating]); \
         rerun tools/extract-block-light.sh"
    )]
    EntryShape { state_id: u32, got: usize },
    #[error("entry at state-id {state_id} has {field} = {value}; must be in 0..=15")]
    OutOfRange {
        state_id: u32,
        field: &'static str,
        value: i64,
    },
    #[error("entry at state-id {state_id} has propagates_sky = {value}; must be 0 or 1")]
    InvalidBool { state_id: u32, value: i64 },
    #[error("entry at state-id {state_id} has suffocating = {value}; must be 0 or 1")]
    InvalidSuffocating { state_id: u32, value: i64 },
}

/// Per-state light metadata, indexed by global block-state id (the
/// same id space `mc_data::blocks::BlockStateReport::id` uses).
#[derive(Debug, Clone)]
pub struct BlockLightTable {
    /// Vanilla version string the table was extracted from
    /// (e.g. `"26.1.2"`). Surfaced in logs for sanity-checking that
    /// the table matches the `blocks.json` shipped alongside it.
    pub version: String,
    /// Length is `max_state_id + 1`; index = global state-id.
    emission: Vec<u8>,
    opacity: Vec<u8>,
    propagates_sky: Vec<bool>,
    suffocating: Vec<bool>,
    /// Six four-bit boundary masks for slab/stair light occlusion.
    faces: Vec<u32>,
}

impl BlockLightTable {
    #[must_use]
    pub fn len(&self) -> usize {
        self.emission.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.emission.is_empty()
    }

    /// Emission (luminance) in `0..=15` for the given state-id, or
    /// `None` if the id is out of range.
    #[must_use]
    pub fn emission(&self, state_id: u32) -> Option<u8> {
        self.emission.get(state_id as usize).copied()
    }

    /// Opacity (`getLightDampening` in vanilla) in `0..=15` for the
    /// given state-id, or `None` if the id is out of range.
    #[must_use]
    pub fn opacity(&self, state_id: u32) -> Option<u8> {
        self.opacity.get(state_id as usize).copied()
    }

    /// Whether sky-light passes through this state.
    #[must_use]
    pub fn propagates_sky(&self, state_id: u32) -> Option<bool> {
        self.propagates_sky.get(state_id as usize).copied()
    }

    #[must_use]
    pub fn suffocating(&self, state_id: u32) -> Option<bool> {
        self.suffocating.get(state_id as usize).copied()
    }

    #[must_use]
    pub fn faces(&self, state_id: u32) -> u32 {
        self.faces.get(state_id as usize).copied().unwrap_or(0)
    }

    #[must_use]
    pub fn face_occludes(&self, from: u32, to: u32, axis: usize, positive: bool) -> bool {
        let side = axis * 2 + usize::from(positive);
        ((self.faces(from) >> (side * 4)) | (self.faces(to) >> ((side ^ 1) * 4))) & 15 == 15
    }

    /// Build a table from in-memory arrays; used by tests and by
    /// callers that don't want to stage a filesystem layout.
    /// Panics if the three arrays have different lengths.
    #[must_use]
    pub fn from_arrays(
        version: impl Into<String>,
        emission: Vec<u8>,
        opacity: Vec<u8>,
        propagates_sky: Vec<bool>,
    ) -> Self {
        assert_eq!(emission.len(), opacity.len());
        assert_eq!(emission.len(), propagates_sky.len());
        let suffocating = opacity.iter().map(|opacity| *opacity == 15).collect();
        Self {
            version: version.into(),
            faces: vec![0; emission.len()],
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }

    #[must_use]
    pub fn from_arrays_with_suffocating(
        version: impl Into<String>,
        emission: Vec<u8>,
        opacity: Vec<u8>,
        propagates_sky: Vec<bool>,
        suffocating: Vec<bool>,
    ) -> Self {
        assert_eq!(emission.len(), opacity.len());
        assert_eq!(emission.len(), propagates_sky.len());
        assert_eq!(emission.len(), suffocating.len());
        Self {
            version: version.into(),
            faces: vec![0; emission.len()],
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }

    /// Append one full, opaque, non-emitting block state after a validated
    /// frozen registry.
    pub fn append_opaque_state(&mut self) {
        self.emission.push(0);
        self.opacity.push(15);
        self.propagates_sky.push(false);
        self.suffocating.push(true);
        self.faces.push(0);
    }

    /// Build a runtime table from `blocks.json` when the exact vanilla
    /// light oracle is absent. Unknown blocks are treated as opaque and
    /// non-emissive, which keeps caves and terrain sane; known air,
    /// transparent, and light-emitting block families get small explicit
    /// overrides.
    #[must_use]
    pub fn conservative_from_blocks_report(report: &[BlockReport]) -> Self {
        let len = report
            .iter()
            .flat_map(|block| block.states.iter().map(|state| state.id as usize))
            .max()
            .map_or(0, |id| id + 1);
        let mut emission = vec![0; len];
        let mut opacity = vec![15; len];
        let mut propagates_sky = vec![false; len];
        let mut suffocating = vec![false; len];

        for block in report {
            let path = block.id.path();
            let chest = path == "chest" || path.ends_with("_chest");
            for state in &block.states {
                let idx = state.id as usize;
                emission[idx] = conservative_emission(path, &state.properties);
                // Vanilla chests are sub-cube shapes: opacity 0, or 1 when
                // waterlogged (26.1.2 block-light report) — never 15.
                let op = if chest {
                    u8::from(
                        state
                            .properties
                            .get("waterlogged")
                            .is_some_and(|value| value == "true"),
                    )
                } else {
                    conservative_opacity(path)
                };
                opacity[idx] = op;
                propagates_sky[idx] = op == 0;
                suffocating[idx] = op == 15;
            }
        }

        Self {
            version: "blocks-report-conservative".to_string(),
            faces: shape_faces(report, len),
            emission,
            opacity,
            propagates_sky,
            suffocating,
        }
    }
}

fn shape_faces(report: &[BlockReport], len: usize) -> Vec<u32> {
    let mut faces = vec![0; len];
    let shapes = crate::collision_shapes::vanilla_collision_shapes();
    for block in report {
        let name = block.id.path();
        if !name.ends_with("_slab") && !name.ends_with("_stairs") {
            continue;
        }
        for state in &block.states {
            let Some(mask) = faces.get_mut(state.id as usize) else {
                continue;
            };
            let props: Vec<_> = state
                .properties
                .iter()
                .map(|(k, v)| (k.clone(), v.clone()))
                .collect();
            if let Some(shape) = shapes.get_for_state(state.id, &block.id, &props) {
                for axis in 0..3 {
                    for positive in [false, true] {
                        *mask |= u32::from(shape.face_quadrants(axis, positive))
                            << ((axis * 2 + usize::from(positive)) * 4);
                    }
                }
            }
        }
    }
    faces
}

fn conservative_opacity(path: &str) -> u8 {
    if matches!(
        path,
        "kelp" | "kelp_plant" | "chorus_flower" | "chorus_plant"
    ) {
        return 1;
    }

    if matches!(path, "air" | "cave_air" | "void_air")
        || path.contains("glass")
        || path.contains("pane")
        || path.contains("water")
        || path.contains("ice")
        || path.contains("leaves")
        || path.ends_with("torch")
        || matches!(path, "lantern" | "soul_lantern")
        || is_transparent_plant(path)
        || is_transparent_crop(path)
        || path == "sugar_cane"
        || path == "cactus"
        || path.contains("flower")
        || path.contains("sapling")
        || path.contains("mushroom")
        || matches!(
            path,
            "short_grass" | "tall_grass" | "seagrass" | "tall_seagrass"
        )
        || path.contains("fern")
        || path.contains("vine")
        || path.contains("roots")
        || path.contains("bush")
        || path.contains("carpet")
        || path.contains("rail")
        || path.contains("sign")
        || path.contains("button")
        || path.contains("pressure_plate")
        || path.contains("ladder")
        || path.contains("chain")
        || path.contains("candle")
        || path.contains("fire")
    {
        0
    } else {
        15
    }
}

fn is_transparent_crop(path: &str) -> bool {
    matches!(
        path,
        "wheat"
            | "carrots"
            | "potatoes"
            | "beetroots"
            | "torchflower_crop"
            | "pitcher_crop"
            | "melon_stem"
            | "attached_melon_stem"
            | "pumpkin_stem"
            | "attached_pumpkin_stem"
            | "sweet_berry_bush"
            | "nether_wart"
            | "cocoa"
            | "bamboo"
    )
}

fn is_transparent_plant(path: &str) -> bool {
    matches!(
        path,
        "poppy"
            | "dandelion"
            | "blue_orchid"
            | "allium"
            | "azure_bluet"
            | "red_tulip"
            | "orange_tulip"
            | "white_tulip"
            | "pink_tulip"
            | "oxeye_daisy"
            | "cornflower"
            | "lily_of_the_valley"
            | "wither_rose"
            | "open_eyeblossom"
            | "closed_eyeblossom"
            | "torchflower"
            | "pitcher_plant"
            | "lilac"
            | "rose_bush"
            | "peony"
            | "sunflower"
    )
}

fn conservative_emission(path: &str, props: &std::collections::BTreeMap<String, String>) -> u8 {
    if props.get("lit").is_some_and(|v| v == "false") {
        return 0;
    }
    if matches!(path, "torchflower" | "torchflower_crop") {
        return 0;
    }
    if matches!(path, "cave_vines" | "cave_vines_plant")
        && props.get("berries").is_some_and(|value| value == "true")
    {
        return 14;
    }
    if path.contains("redstone_torch") {
        return 7;
    }
    if path.contains("soul_torch") || path.contains("soul_lantern") || path.contains("soul_fire") {
        return 10;
    }
    if path.contains("torch") {
        return 14;
    }
    if path.contains("glowstone")
        || path.contains("sea_lantern")
        || path.contains("jack_o_lantern")
        || path.contains("redstone_lamp")
        || path == "lava"
        || path.contains("lava")
        || path.contains("fire")
        || path.contains("lantern")
    {
        return 15;
    }
    if path.contains("candle") {
        return props
            .get("candles")
            .and_then(|v| v.parse::<u8>().ok())
            .map_or(3, |n| n.saturating_mul(3).min(12));
    }
    // Vanilla 26.1.2 report: every ender_chest state emits 7.
    if path == "ender_chest" {
        return 7;
    }
    0
}

#[derive(Deserialize)]
struct RawTable {
    version: String,
    max_state_id: u32,
    entries: Vec<Vec<i64>>,
}

/// Read and parse the table at `path`.
pub fn load(path: impl AsRef<Path>) -> Result<BlockLightTable, BlockLightError> {
    let path = path.as_ref();
    if !path.is_file() {
        return Err(BlockLightError::Missing(path.to_path_buf()));
    }
    let raw: RawTable = read_json_file(
        path,
        &|path, source| BlockLightError::Io { path, source },
        &|path, source| BlockLightError::Parse { path, source },
    )?;
    parse_raw(raw)
}

fn parse_raw(raw: RawTable) -> Result<BlockLightTable, BlockLightError> {
    let expected = raw.max_state_id as usize + 1;
    if raw.entries.len() != expected {
        return Err(BlockLightError::LengthMismatch {
            entries: raw.entries.len(),
            max_state_id: raw.max_state_id,
            expected,
        });
    }

    let mut emission = Vec::with_capacity(expected);
    let mut opacity = Vec::with_capacity(expected);
    let mut propagates_sky = Vec::with_capacity(expected);
    let mut suffocating = Vec::with_capacity(expected);
    for (state_id, e) in raw.entries.iter().enumerate() {
        if e.len() != 4 {
            return Err(BlockLightError::EntryShape {
                state_id: state_id as u32,
                got: e.len(),
            });
        }
        let em = e[0];
        let op = e[1];
        let ps = e[2];
        let sf = e[3];
        if !(0..=15).contains(&em) {
            return Err(BlockLightError::OutOfRange {
                state_id: state_id as u32,
                field: "emission",
                value: em,
            });
        }
        if !(0..=15).contains(&op) {
            return Err(BlockLightError::OutOfRange {
                state_id: state_id as u32,
                field: "opacity",
                value: op,
            });
        }
        if !(0..=1).contains(&ps) {
            return Err(BlockLightError::InvalidBool {
                state_id: state_id as u32,
                value: ps,
            });
        }
        if !(0..=1).contains(&sf) {
            return Err(BlockLightError::InvalidSuffocating {
                state_id: state_id as u32,
                value: sf,
            });
        }
        emission.push(em as u8);
        opacity.push(op as u8);
        propagates_sky.push(ps == 1);
        suffocating.push(sf == 1);
    }

    Ok(BlockLightTable {
        version: raw.version,
        faces: shape_faces(&crate::blocks::solaris_required_blocks_report(), expected),
        emission,
        opacity,
        propagates_sky,
        suffocating,
    })
}

#[cfg(test)]
#[path = "block_light_tests.rs"]
mod tests;
