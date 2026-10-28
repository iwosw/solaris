//! Version-ordered, offline NBT upgrades for Java Edition worlds and auxiliary data.
//!
//! This is not a runtime compatibility reader. Importers must upgrade the complete
//! record before invoking a 26.1.2 reader. Unknown remainder fields are preserved;
//! a known field whose migration cannot be represented safely is rejected with a
//! version/fix/field receipt. On error, discard the partially transformed record:
//! its original DataVersion is deliberately not changed. No world data is written.
//! Chunk height/blending fixes use vanilla's `__context.dimension`, and the
//! pre-1.18 height fix also uses `__context.generator` (the generator type).
//! Importers must supply the actual world context, not infer a noise generator.

mod block_entities;
mod chunks;
mod entities;
mod items;
mod metadata;
mod nbt;
mod old_block_states;
mod old_chunks;
mod old_entities;
mod old_items;
mod old_metadata;
mod old_payloads;
mod old_player_map;
mod old_worldgen;
mod text;
mod v263;
mod walk;
pub use v263::{prepare_export, requires_26_3};

/// Native 26.3 source/export version, from the local official version.json.
pub const NATIVE_26_3_DATA_VERSION: i32 = 5023;

#[cfg(test)]
mod tests;

use mc_nbt::Tag;
use thiserror::Error;

pub use super::chunk_nbt::TARGET_DATA_VERSION;
/// Oldest supported Java Edition world version (1.15).
pub const MIN_DATA_VERSION: i32 = 2_230;

/// The root schema to upgrade. Entity-region records include the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordKind {
    Chunk,
    EntityRegion,
    Entity,
    Player,
    Level,
    Item,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum NodeKind {
    Chunk,
    EntityRegion,
    Entity,
    Player,
    Level,
    Item,
    BlockEntity,
    BlockState,
    Components,
    Text,
}

/// A vanilla fix which changed at least one typed field in this record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StepReceipt {
    pub version: i32,
    pub fix: &'static str,
}

/// Data retained verbatim because it is opaque to this upgrader.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreservationReceipt {
    pub path: String,
    pub reason: String,
}

/// Pure-NBT output of vanilla's level.dat split. Importers must consume these
/// alongside the level record; paths are vanilla world-relative destinations.
#[derive(Debug, Clone, PartialEq)]
pub struct SavedDataRecord {
    pub path: String,
    pub tag: Tag,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct UpgradeReport {
    pub from: i32,
    pub to: i32,
    pub steps: Vec<StepReceipt>,
    pub preserved: Vec<PreservationReceipt>,
    pub saved_data: Vec<SavedDataRecord>,
}

impl UpgradeReport {
    pub(super) fn applied(&mut self, version: i32, fix: &'static str) {
        let receipt = StepReceipt { version, fix };
        if !self.steps.contains(&receipt) {
            self.steps.push(receipt);
        }
    }

    pub(super) fn preserved(&mut self, path: impl AsRef<str>, reason: impl AsRef<str>) {
        let path = path.as_ref();
        let reason = reason.as_ref();
        if !self
            .preserved
            .iter()
            .any(|receipt| receipt.path == path && receipt.reason == reason)
        {
            self.preserved.push(PreservationReceipt {
                path: path.into(),
                reason: reason.into(),
            });
        }
    }
}

/// Fail-closed migration receipt; never interpret the record as 26.1.2 on error.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum UpgradeError {
    #[error(
        "unsupported Minecraft DataVersion {0}; supported Java Edition world records are 1.15 ({MIN_DATA_VERSION}) through 26.1.2 ({TARGET_DATA_VERSION}), or native 26.3 ({NATIVE_26_3_DATA_VERSION})"
    )]
    UnsupportedVersion(i32),
    #[error("record DataVersion {actual} differs from supplied source version {supplied}")]
    VersionMismatch { supplied: i32, actual: i32 },
    #[error("malformed NBT at {path}: {reason}")]
    Malformed { path: String, reason: String },
    #[error("cannot safely apply {fix} at DataVersion {version} to {path}: {reason}")]
    UnsupportedField {
        version: i32,
        fix: &'static str,
        path: String,
        reason: String,
    },
}

impl UpgradeError {
    pub(super) fn unsupported(
        version: i32,
        fix: &'static str,
        path: impl Into<String>,
        reason: impl Into<String>,
    ) -> Self {
        Self::UnsupportedField {
            version,
            fix,
            path: path.into(),
            reason: reason.into(),
        }
    }

    pub(super) fn malformed(path: impl Into<String>, reason: impl Into<String>) -> Self {
        Self::Malformed {
            path: path.into(),
            reason: reason.into(),
        }
    }
}

// Historical typed auxiliary fixers, before the region-data support floor.
// Sources use revision zero: later revisions at the same integer still run.
// Owners inspect report.from to skip only their revision-zero registrations.
const LEGACY_AUX_VERSIONS: &[i32] = &[
    100, 101, 102, 105, 106, 107, 108, 109, 110, 111, 113, 135, 143, 147, 165, 502, 700, 701, 702,
    703, 704, 705, 804, 806, 808, 813, 820, 1022, 1125, 1450, 1451, 1456, 1458, 1460, 1474, 1475,
    1480, 1483, 1484, 1486, 1487, 1488, 1490, 1494, 1500, 1502, 1510, 1515, 1800, 1802, 1803, 1904,
    1914, 1917, 1918, 1928, 1948, 1953, 1955, 1963, 2100, 2209, 2501, 2503, 2505, 2508, 2509, 2511,
    2514, 2516, 2518, 2523, 2527, 2528, 2529, 2531, 2533, 2535, 2537, 2538, 2550,
];

// Historical registrations are separate so the established 2860+ chain stays
// unchanged. AddNewChoices registrations only extend schemas, not NBT payloads.
const OLD_VERSIONS: &[i32] = &[
    2679, 2680, 2690, 2691, 2693, 2696, 2700, 2701, 2702, 2707, 2717, 2825, 2831, 2832, 2833, 2838,
    2841, 2842, 2843, 2852,
];

// Every schema registration after 2860, in vanilla order. Same-DataVersion
// subversions are applied in their owning function's registration order.
const VERSIONS: &[i32] = &[
    2967, 2970, 3076, 3077, 3078, 3081, 3082, 3083, 3084, 3086, 3087, 3090, 3093, 3094, 3097, 3108,
    3201, 3202, 3203, 3204, 3209, 3214, 3319, 3322, 3325, 3326, 3327, 3328, 3438, 3439, 3440, 3447,
    3448, 3450, 3451, 3459, 3564, 3565, 3566, 3568, 3682, 3683, 3685, 3689, 3692, 3799, 3800, 3803,
    3807, 3808, 3809, 3812, 3813, 3814, 3816, 3818, 3820, 3825, 3828, 3833, 3938, 3939, 3943, 3945,
    4054, 4055, 4057, 4059, 4061, 4064, 4067, 4068, 4070, 4071, 4081, 4173, 4175, 4176, 4180, 4181,
    4187, 4290, 4291, 4292, 4293, 4294, 4295, 4296, 4297, 4299, 4300, 4301, 4302, 4303, 4305, 4306,
    4307, 4309, 4311, 4312, 4314, 4420, 4421, 4424, 4531, 4532, 4533, 4535, 4537, 4541, 4543, 4544,
    4548, 4648, 4649, 4650, 4651, 4656, 4657, 4658, 4661, 4763, 4766, 4769, 4771, 4772, 4773,
];

/// Upgrade one complete root record in place. `level.dat` may use its `Data`
/// wrapper; all other records are unwrapped compounds. Source versions below
/// [`MIN_DATA_VERSION`] or above 4790 are rejected, except native 26.3 (5023).
/// Successful calls normalize to the native reader's 4790 block-state spelling.
/// Missing DataVersion is allowed because `from` is explicit caller input.
/// On an error the caller must discard the record (see module documentation).
pub fn upgrade_record(
    tag: &mut Tag,
    kind: RecordKind,
    from: i32,
) -> Result<UpgradeReport, UpgradeError> {
    if (from > TARGET_DATA_VERSION && from != NATIVE_26_3_DATA_VERSION)
        || (from < MIN_DATA_VERSION && !matches!(kind, RecordKind::Player | RecordKind::Item))
    {
        return Err(UpgradeError::UnsupportedVersion(from));
    }
    if nbt::fields(tag).is_none() {
        return Err(UpgradeError::malformed("$", "expected compound record"));
    }
    let node = match kind {
        RecordKind::Chunk => NodeKind::Chunk,
        RecordKind::EntityRegion => NodeKind::EntityRegion,
        RecordKind::Entity => NodeKind::Entity,
        RecordKind::Player => NodeKind::Player,
        RecordKind::Level => NodeKind::Level,
        RecordKind::Item => NodeKind::Item,
    };
    let wrapped = kind == RecordKind::Level && nbt::get(tag, "Data").is_some();
    let outer_version = if wrapped {
        nbt::get(tag, "DataVersion")
            .map(|version| {
                nbt::int(version)
                    .ok_or_else(|| UpgradeError::malformed("DataVersion", "expected integer"))
            })
            .transpose()?
    } else {
        None
    };
    if let Some(actual) = outer_version
        && actual != from
    {
        return Err(UpgradeError::VersionMismatch {
            supplied: from,
            actual,
        });
    }
    let root = if wrapped {
        nbt::get_mut(tag, "Data").expect("checked Data field")
    } else {
        &mut *tag
    };
    if nbt::fields(root).is_none() {
        return Err(UpgradeError::malformed("Data", "expected compound"));
    }
    if let Some(version) = nbt::get(root, "DataVersion") {
        let actual = nbt::int(version)
            .ok_or_else(|| UpgradeError::malformed("DataVersion", "expected integer"))?;
        if actual != from {
            return Err(UpgradeError::VersionMismatch {
                supplied: from,
                actual,
            });
        }
    }
    let mut report = UpgradeReport {
        from,
        to: TARGET_DATA_VERSION,
        ..UpgradeReport::default()
    };
    if from < 2586 {
        for &version in LEGACY_AUX_VERSIONS.iter().filter(|&&version| {
            version > from || (version == from && matches!(version, 808 | 1451 | 2511))
        }) {
            walk::visit(root, node, version, &mut report)?;
        }
    }
    // DFU source keys use from*10; revisions>0 at that same integer version
    // still run. Owning functions suppress their revision0 case in this branch.
    for &version in OLD_VERSIONS.iter().chain(VERSIONS).filter(|&&version| {
        version > from || (version == from && matches!(version, 3439 | 3564 | 3807 | 3808 | 3818))
    }) {
        walk::visit(root, node, version, &mut report)?;
    }
    // Schema-scoped normalization also upgrades legacy pot lists to named
    // templates. Never rewrite similarly named fields inside opaque payloads.
    walk::visit(root, node, NATIVE_26_3_DATA_VERSION, &mut report)?;
    nbt::set(root, "DataVersion", Tag::Int(TARGET_DATA_VERSION));
    if outer_version.is_some() {
        nbt::set(tag, "DataVersion", Tag::Int(TARGET_DATA_VERSION));
    }
    Ok(report)
}

pub fn upgrade_chunk(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::Chunk, from)
}

/// Upgrade a chunk using its externally known dimension and optional generator
/// type, without persisting vanilla's transient `__context` field.
/// Any original context field is restored, including when migration fails.
pub fn upgrade_chunk_in_dimension(
    tag: &mut Tag,
    from: i32,
    dimension: &str,
    generator: Option<&str>,
) -> Result<UpgradeReport, UpgradeError> {
    let previous = nbt::take(tag, "__context");
    let mut context = nbt::compound(vec![("dimension", Tag::String(dimension.into()))]);
    if let Some(generator) = generator {
        nbt::set(&mut context, "generator", Tag::String(generator.into()));
    }
    nbt::set(tag, "__context", context);
    let result = upgrade_chunk(tag, from);
    nbt::remove(tag, "__context");
    if let Some(previous) = previous {
        nbt::set(tag, "__context", previous);
    }
    result
}
pub fn upgrade_entity_region(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::EntityRegion, from)
}
pub fn upgrade_entity(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::Entity, from)
}
pub fn upgrade_player(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::Player, from)
}
/// Upgrade a saved map-data file through the current schema, retaining pixels.
pub fn upgrade_map_data(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    old_player_map::upgrade_map_data(tag, from)
}
pub fn upgrade_level(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::Level, from)
}
pub fn upgrade_item(tag: &mut Tag, from: i32) -> Result<UpgradeReport, UpgradeError> {
    upgrade_record(tag, RecordKind::Item, from)
}

#[cfg(test)]
mod v263_tests;
