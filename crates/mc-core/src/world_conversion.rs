//! Explicit, offline Anvil interchange. Serving never reads or writes Anvil.
//! Saved data without a pure-NBT fix retains its version and original bytes;
//! only vanilla's storage path is relocated. POI files remain wholly opaque.
//! Typed metadata whose payload no DFU fixer rewrote since 4772 (world clocks,
//! wandering trader, scheduled/custom events) is accepted at its stored 4772
//! stamp exactly as vanilla decodes it; everything else still requires 4790.
mod census;
#[cfg(test)]
mod census_tests;
pub use census::{CensusReceipt, DimensionCensus, SettlementCensus, census_native};
mod export;
#[cfg(test)]
mod export_tests;
#[cfg(test)]
mod falling_import_tests;
mod import;
#[cfg(test)]
mod old_world_import_tests;
#[cfg(test)]
mod roundtrip_tests;
#[cfg(test)]
mod tests;

use crate::{CoreConfig, CoreError};
use mc_nbt::Tag;
use mc_world::anvil::upgrade::{RecordKind, UpgradeReport};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub(super) struct ConversionStats {
    pub tick: u64,
    pub durable_tick: u64,
    pub chunks: u64,
    pub non_full_empty_chunks: u64,
    pub non_full_non_air_chunks: u64,
    pub non_full_air_with_payload_chunks: u64,
    pub regenerated_chunk_tags: BTreeMap<String, u64>,
    pub regenerated_chunk_loss_tags: BTreeMap<String, u64>,
    pub regenerated_structure_losses: u64,
    pub dropped_noop_fluid_ticks: u64,
    pub dropped_foreign_scheduled_ticks: u64,
    pub deduplicated_scheduled_ticks: u64,
    pub duplicate_uuid_drops: Vec<DuplicateUuidDrop>,
    pub known_gaps: Vec<String>,
    pub entities: u64,
    pub players: u64,
    pub opaque_files: u64,
    pub reminted_ids: u64,
    pub opaque_retention: Vec<String>,
    pub upgrades: Vec<RecordUpgradeReceipt>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub(super) struct DuplicateUuidDrop {
    pub source: PathBuf,
    pub chunk: [i32; 2],
    pub entity_index: usize,
    pub uuid: String,
    pub staged_nbt: PathBuf,
}

/// Record-local evidence from the pure-NBT upgrade, excluding its NBT payloads.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordUpgradeReceipt {
    pub record: String,
    pub kind: String,
    pub from: i32,
    pub to: i32,
    pub steps: Vec<UpgradeFixReceipt>,
    pub preserved: Vec<UpgradePreservationReceipt>,
    pub saved_data: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradeFixReceipt {
    pub version: i32,
    pub fix: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpgradePreservationReceipt {
    pub path: String,
    pub reason: String,
}

impl RecordUpgradeReceipt {
    fn from_report(record: String, kind: RecordKind, report: &UpgradeReport) -> Self {
        Self {
            record,
            kind: match kind {
                RecordKind::Chunk => "chunk",
                RecordKind::EntityRegion => "entity_region",
                RecordKind::Entity => "entity",
                RecordKind::Player => "player",
                RecordKind::Level => "level",
                RecordKind::Item => "item",
            }
            .into(),
            from: report.from,
            to: report.to,
            steps: report
                .steps
                .iter()
                .map(|step| UpgradeFixReceipt {
                    version: step.version,
                    fix: step.fix.into(),
                })
                .collect(),
            preserved: report
                .preserved
                .iter()
                .map(|field| UpgradePreservationReceipt {
                    path: field.path.clone(),
                    reason: field.reason.clone(),
                })
                .collect(),
            saved_data: report
                .saved_data
                .iter()
                .map(|data| data.path.clone())
                .collect(),
        }
    }
}

/// Durable operator evidence; only a fully published destination reports PASS.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConversionReceipt {
    pub status: String,
    pub operation: String,
    pub source: PathBuf,
    pub destination: PathBuf,
    pub data_version: i32,
    pub native_schema: u32,
    pub checkpoint_tick: Option<u64>,
    pub replay_tick: Option<u64>,
    pub dimensions: Vec<String>,
    pub counts: BTreeMap<String, u64>,
    pub input_hashes: BTreeMap<String, String>,
    pub output_hashes: BTreeMap<String, String>,
    pub hash_algorithm: String,
    pub normalizations: Vec<String>,
    pub opaque_retention: Vec<String>,
    pub upgrades: Vec<RecordUpgradeReceipt>,
    pub known_gaps: Vec<String>,
    pub error: Option<String>,
}

pub fn import_anvil(
    config: &CoreConfig,
    source: &Path,
    destination: &Path,
    receipt: &Path,
) -> Result<ConversionReceipt, CoreError> {
    convert(
        "import-anvil",
        config,
        source,
        destination,
        receipt,
        import::convert,
    )
}
pub fn export_anvil(
    config: &CoreConfig,
    source: &Path,
    destination: &Path,
    receipt: &Path,
) -> Result<ConversionReceipt, CoreError> {
    convert(
        "export-anvil",
        config,
        source,
        destination,
        receipt,
        export::convert,
    )
}

fn convert(
    operation: &str,
    config: &CoreConfig,
    source: &Path,
    destination: &Path,
    receipt_path: &Path,
    adapter: fn(&CoreConfig, &Path, &Path) -> Result<ConversionStats, CoreError>,
) -> Result<ConversionReceipt, CoreError> {
    // Receipt paths are an output too: validate before any refusal can emit one.
    let receipt_parent = receipt_path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let receipt_absolute = fs::canonicalize(receipt_parent).map_err(fail)?.join(
        receipt_path
            .file_name()
            .ok_or_else(|| fail("receipt has no filename"))?,
    );
    for root in [source, destination] {
        if let Ok(root) = fs::canonicalize(root)
            && receipt_absolute.starts_with(root)
        {
            return Err(fail("receipt must be outside source and destination"));
        }
    }
    let mut receipt = ConversionReceipt {
        status: "FAIL".into(),
        operation: operation.into(),
        source: source.into(),
        destination: destination.into(),
        data_version: 4790,
        native_schema: mc_world::native::NATIVE_PROTOTYPE_SCHEMA,
        checkpoint_tick: None,
        replay_tick: None,
        dimensions: vec![
            "minecraft:overworld".into(),
            "minecraft:the_nether".into(),
            "minecraft:the_end".into(),
        ],
        counts: BTreeMap::new(),
        input_hashes: BTreeMap::new(),
        output_hashes: BTreeMap::new(),
        hash_algorithm: "CRC32".into(),
        normalizations: Vec::new(),
        opaque_retention: Vec::new(),
        upgrades: Vec::new(),
        known_gaps: Vec::new(),
        error: None,
    };
    let mut staging = None;
    let mut receipt_allowed = true;
    let result = (|| {
        let source = fs::canonicalize(source).map_err(fail)?;
        if !source.is_dir() {
            return Err(fail("source must be a directory"));
        }
        let lease = File::open(&source).map_err(fail)?;
        lease.try_lock().map_err(fail)?;
        let _java_lease = if operation == "import-anvil" {
            Some(JavaLease::acquire(&source)?)
        } else {
            None
        };
        if destination.try_exists().map_err(fail)? {
            return Err(fail("destination already exists"));
        }
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let parent = fs::canonicalize(parent).map_err(fail)?;
        let destination = parent.join(
            destination
                .file_name()
                .ok_or_else(|| fail("destination has no filename"))?,
        );
        if destination
            .file_name()
            .is_some_and(|name| name.to_string_lossy().starts_with(".solaris-conversion-"))
        {
            return Err(fail(
                "destination uses reserved unpublished conversion staging name",
            ));
        }
        if destination.starts_with(&source) {
            return Err(fail("destination must not be inside source"));
        }
        let receipt_parent = receipt_path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."));
        let receipt_absolute = fs::canonicalize(receipt_parent).map_err(fail)?.join(
            receipt_path
                .file_name()
                .ok_or_else(|| fail("receipt has no filename"))?,
        );
        if receipt_absolute.starts_with(&source) || receipt_absolute.starts_with(&destination) {
            receipt_allowed = false;
            return Err(fail("receipt must be outside source and destination"));
        }
        receipt.input_hashes = hashes(&source)?;
        let stage = parent.join(format!(".solaris-conversion-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&stage).map_err(fail)?;
        staging = Some(stage.clone());
        let stats = adapter(config, &source, &stage)?;
        if receipt.input_hashes != hashes(&source)? {
            return Err(fail("source changed during offline conversion"));
        }
        if !stats.duplicate_uuid_drops.is_empty() {
            let archive = receipt_absolute.with_file_name(format!(
                "{}.dropped-entity-nbt",
                receipt_absolute
                    .file_name()
                    .expect("validated receipt filename")
                    .to_string_lossy(),
            ));
            fs::create_dir(&archive).map_err(fail)?;
            for dropped in &stats.duplicate_uuid_drops {
                let artifact = archive.join(
                    dropped
                        .staged_nbt
                        .file_name()
                        .ok_or_else(|| fail("dropped entity NBT has no filename"))?,
                );
                let mut original = File::open(stage.join(&dropped.staged_nbt)).map_err(fail)?;
                let mut preserved = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&artifact)
                    .map_err(fail)?;
                std::io::copy(&mut original, &mut preserved).map_err(fail)?;
                preserved.sync_all().map_err(fail)?;
                receipt.known_gaps.push(format!(
                    "PRESERVATION LOSS: {}#chunk({}, {}) entity-index {} UUID {}: duplicate UUID body dropped; deterministic first body kept, references unchanged; original NBT: {}",
                    dropped.source.display(), dropped.chunk[0], dropped.chunk[1],
                    dropped.entity_index, dropped.uuid, artifact.display(),
                ));
            }
            crate::journal::fsync_dir(&archive).map_err(fail)?;
            fs::remove_dir_all(stage.join("duplicate-uuid-dropped")).map_err(fail)?;
        }
        receipt.output_hashes = hashes(&stage)?;
        sync_tree(&stage)?;
        receipt.checkpoint_tick = Some(stats.durable_tick);
        receipt.replay_tick = Some(stats.tick);
        for (key, value) in [
            ("chunks", stats.chunks),
            ("non_full_empty_chunks", stats.non_full_empty_chunks),
            ("non_full_non_air_chunks", stats.non_full_non_air_chunks),
            (
                "non_full_air_with_payload_chunks",
                stats.non_full_air_with_payload_chunks,
            ),
            (
                "regenerated_structure_losses",
                stats.regenerated_structure_losses,
            ),
            ("dropped_noop_fluid_ticks", stats.dropped_noop_fluid_ticks),
            (
                "dropped_foreign_scheduled_ticks",
                stats.dropped_foreign_scheduled_ticks,
            ),
            (
                "deduplicated_scheduled_ticks",
                stats.deduplicated_scheduled_ticks,
            ),
            (
                "duplicate_uuid_dropped",
                stats.duplicate_uuid_drops.len() as u64,
            ),
            ("entities", stats.entities),
            ("players", stats.players),
            ("opaque_files", stats.opaque_files),
            ("reminted_ids", stats.reminted_ids),
        ] {
            receipt.counts.insert(key.into(), value);
        }
        receipt.counts.extend(
            stats
                .regenerated_chunk_tags
                .into_iter()
                .map(|(tag, count)| (format!("regenerated_chunk_tag.{tag}"), count)),
        );
        receipt.counts.extend(
            stats
                .regenerated_chunk_loss_tags
                .into_iter()
                .map(|(tag, count)| (format!("preservation_loss.{tag}"), count)),
        );
        receipt.known_gaps.extend(stats.known_gaps);
        receipt.opaque_retention = stats.opaque_retention;
        receipt.upgrades = stats.upgrades;
        receipt
            .normalizations
            .push("Anvil payloads use DataVersion 4790 and recovered checkpoint time".into());
        receipt
            .opaque_retention
            .push("unknown NBT fields and safe auxiliary files retained except explicit empty-chunk regeneration and duplicate-UUID dispositions".into());
        // Prove receipt durability before publishing the staged world. A crash
        // between publications leaves PREPARED evidence, never a false PASS.
        receipt.status = "PREPARED".into();
        write_receipt(receipt_path, &receipt)?;
        // Destination admission is checked again immediately before the only publication.
        if destination.try_exists().map_err(fail)? {
            return Err(fail("destination appeared during conversion"));
        }
        rustix::fs::renameat_with(
            rustix::fs::CWD,
            &stage,
            rustix::fs::CWD,
            &destination,
            rustix::fs::RenameFlags::NOREPLACE,
        )
        .map_err(fail)?;
        staging = None;
        crate::journal::fsync_dir(&parent).map_err(fail)?;
        receipt.status = "PASS".into();
        Ok(())
    })();
    if let Err(error) = &result {
        receipt.status = "FAIL".into();
        receipt.error = Some(error.to_string());
    }
    if receipt_allowed {
        write_receipt(receipt_path, &receipt)?;
    }
    if let Some(stage) = staging
        && let Err(cleanup) = fs::remove_dir_all(&stage)
    {
        let original = receipt.error.as_deref().unwrap_or("conversion completed");
        let combined = format!("{original}; staging cleanup {}: {cleanup}", stage.display());
        receipt.status = "FAIL".into();
        receipt.error = Some(combined.clone());
        if receipt_allowed {
            write_receipt(receipt_path, &receipt)?;
        }
        return Err(fail(combined));
    }
    result?;
    Ok(receipt)
}

pub(super) fn fail(error: impl std::fmt::Display) -> CoreError {
    CoreError::Storage(format!("world conversion: {error}"))
}

/// Read-only POSIX record lease interoperating with Java FileChannel.tryLock.
/// A missing session.lock means the world has not been opened by Java.
struct JavaLease {
    _file: Option<File>,
}
impl JavaLease {
    fn acquire(source: &Path) -> Result<Self, CoreError> {
        let file = match File::open(source.join("session.lock")) {
            Ok(file) => Some(file),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(fail(error)),
        };
        if let Some(file) = &file {
            rustix::fs::fcntl_lock(file, rustix::fs::FlockOperation::NonBlockingLockShared)
                .map_err(|error| {
                    fail(format!(
                        "Anvil source must be stopped; POSIX session lock refused: {error}"
                    ))
                })?;
        }
        Ok(Self { _file: file })
    }
}

pub(super) fn safe_extra_path(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    crate::level_metadata::validate_relative_path(path).is_ok()
        && (path == "icon.png"
            || path == "level.dat_old"
            || path == "resourcepacks/resources.zip"
            || path.starts_with("advancements/")
            || path.starts_with("stats/")
            || path.starts_with("data/")
            || path.starts_with("players/")
            || (path.starts_with("dimensions/")
                && (path.contains("/poi/") || path.contains("/data/"))))
}

/// Vanilla4772 storage moves; never rewrite or infer saved-data contents.
pub(super) fn auxiliary_destination(path: &Path) -> Result<PathBuf, CoreError> {
    let name = path
        .to_str()
        .ok_or_else(|| fail("non-UTF8 auxiliary path"))?;
    crate::level_metadata::validate_relative_path(name)?;
    if name == "resources.zip" {
        return Ok("resourcepacks/resources.zip".into());
    }
    // PlayerStorageFileFix moves the complete playerdata directory, including
    // backups. They remain opaque and must never become active player records.
    for prefix in ["playerdata/", "players/data/"] {
        if let Some(filename) = name.strip_prefix(prefix)
            && let Some(stem) = filename.strip_suffix(".dat_old")
        {
            let uuid = uuid::Uuid::parse_str(stem)
                .map_err(|_| fail(format!("{name}: unsupported player backup filename")))?;
            if uuid.is_nil() || stem != uuid.to_string() {
                return Err(fail(format!("{name}: unsupported player backup filename")));
            }
            return Ok(format!("players/data/{filename}").into());
        }
    }
    if let Some(filename) = name.strip_prefix("stats/") {
        return Ok(format!("players/stats/{filename}").into());
    }
    if let Some(filename) = name.strip_prefix("advancements/") {
        return Ok(format!("players/advancements/{filename}").into());
    }
    for (prefix, destination) in [
        ("poi/", "dimensions/minecraft/overworld/poi/"),
        ("DIM-1/poi/", "dimensions/minecraft/the_nether/poi/"),
        ("DIM1/poi/", "dimensions/minecraft/the_end/poi/"),
        (
            "dimensions/minecraft/overworld/poi/",
            "dimensions/minecraft/overworld/poi/",
        ),
        (
            "dimensions/minecraft/the_nether/poi/",
            "dimensions/minecraft/the_nether/poi/",
        ),
        (
            "dimensions/minecraft/the_end/poi/",
            "dimensions/minecraft/the_end/poi/",
        ),
    ] {
        if let Some(filename) = name.strip_prefix(prefix) {
            validate_poi_filename(filename)?;
            return Ok(format!("{destination}{filename}").into());
        }
    }
    for (prefix, destination) in [
        ("data/", "dimensions/minecraft/overworld/data/minecraft/"),
        (
            "DIM-1/data/",
            "dimensions/minecraft/the_nether/data/minecraft/",
        ),
        ("DIM1/data/", "dimensions/minecraft/the_end/data/minecraft/"),
        (
            "data/minecraft/",
            "dimensions/minecraft/overworld/data/minecraft/",
        ),
        (
            "dimensions/minecraft/overworld/data/",
            "dimensions/minecraft/overworld/data/minecraft/",
        ),
        (
            "dimensions/minecraft/the_nether/data/",
            "dimensions/minecraft/the_nether/data/minecraft/",
        ),
        (
            "dimensions/minecraft/the_end/data/",
            "dimensions/minecraft/the_end/data/minecraft/",
        ),
    ] {
        if let Some(filename) = name.strip_prefix(prefix) {
            let mapped = match filename {
                "raids.dat" | "raids_end.dat" => Some("raids.dat"),
                "chunks.dat" | "chunk_tickets.dat" => Some("chunk_tickets.dat"),
                "world_border.dat" => Some("world_border.dat"),
                _ => None,
            };
            if let Some(filename) = mapped {
                return Ok(format!("{destination}{filename}").into());
            }
        }
    }
    for prefix in ["data/", "data/minecraft/"] {
        let Some(filename) = name.strip_prefix(prefix) else {
            continue;
        };
        let destination = match filename {
            "scoreboard.dat" => Some("data/minecraft/scoreboard.dat"),
            "stopwatches.dat" => Some("data/minecraft/stopwatches.dat"),
            "random_sequences.dat" => Some("data/minecraft/random_sequences.dat"),
            "idcounts.dat" => Some("data/minecraft/maps/last_id.dat"),
            _ => None,
        };
        if let Some(destination) = destination {
            return Ok(destination.into());
        }
        if let Some(id) = filename
            .strip_prefix("map_")
            .and_then(|value| value.strip_suffix(".dat"))
            && !id.is_empty()
            && id.bytes().all(|byte| byte.is_ascii_digit())
        {
            return Ok(format!("data/minecraft/maps/{id}.dat").into());
        }
        if let Some(namespace) = filename
            .strip_prefix("command_storage_")
            .and_then(|value| value.strip_suffix(".dat"))
            && !namespace.is_empty()
            && namespace.bytes().all(|byte| {
                byte.is_ascii_lowercase()
                    || byte.is_ascii_digit()
                    || matches!(byte, b'_' | b'.' | b'-')
            })
        {
            let destination = format!("data/{namespace}/command_storage.dat");
            crate::level_metadata::validate_relative_path(&destination)?;
            return Ok(destination.into());
        }
    }
    Ok(path.into())
}

fn validate_poi_filename(filename: &str) -> Result<(), CoreError> {
    let coordinate = |value: &str| {
        let digits = value.strip_prefix('-').unwrap_or(value);
        !digits.is_empty()
            && digits.bytes().all(|byte| byte.is_ascii_digit())
            && (value == "0" || !digits.starts_with('0'))
            && value.parse::<i32>().is_ok()
    };
    let mut parts = filename.split('.');
    let valid = match (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) {
        (Some(kind), Some(x), Some(z), Some(extension), None) => {
            matches!((kind, extension), ("r", "mca") | ("c", "mcc"))
                && coordinate(x)
                && coordinate(z)
        }
        _ => false,
    };
    if !valid {
        return Err(fail(format!(
            "POI {filename}: expected canonical r.X.Z.mca or c.X.Z.mcc filename"
        )));
    }
    Ok(())
}

pub(super) fn files(root: &Path) -> Result<Vec<PathBuf>, CoreError> {
    fn walk(root: &Path, relative: &Path, out: &mut Vec<PathBuf>) -> Result<(), CoreError> {
        let mut entries = fs::read_dir(root.join(relative))
            .map_err(fail)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(fail)?;
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = relative.join(entry.file_name());
            let kind = entry.file_type().map_err(fail)?;
            if kind.is_symlink() {
                return Err(fail(format!("symlink rejected: {}", path.display())));
            }
            if kind.is_dir() {
                walk(root, &path, out)?;
            } else if kind.is_file() {
                crate::level_metadata::validate_relative_path(
                    path.to_str().ok_or_else(|| fail("non-UTF8 source path"))?,
                )?;
                out.push(path);
            } else {
                return Err(fail(format!(
                    "non-regular source entry: {}",
                    path.display()
                )));
            }
        }
        Ok(())
    }
    let mut out = Vec::new();
    walk(root, Path::new(""), &mut out)?;
    Ok(out)
}
fn hashes(root: &Path) -> Result<BTreeMap<String, String>, CoreError> {
    let mut out = BTreeMap::new();
    for path in files(root)? {
        // Closing any descriptor for this inode releases a process POSIX lease.
        // Session lock bytes are owner coordination, not convertible world data.
        if path == Path::new("session.lock") {
            continue;
        }
        let mut input = File::open(root.join(&path)).map_err(fail)?;
        let mut hasher = crc32fast::Hasher::new();
        let mut buffer = [0u8; 65536];
        loop {
            let n = input.read(&mut buffer).map_err(fail)?;
            if n == 0 {
                break;
            }
            hasher.update(&buffer[..n]);
        }
        out.insert(
            path.to_string_lossy().into_owned(),
            format!("{:08x}", hasher.finalize()),
        );
    }
    Ok(out)
}
pub(super) fn read_nbt(path: &Path) -> Result<Tag, CoreError> {
    // Vanilla `SavedDataStorage.readTagFromDisk` sniffs the gzip magic
    // (`isGzip` on the first two bytes) and falls back to raw named NBT.
    // Field proof: `data/minecraft/maps/last_id.dat` in genuine 26.1.2
    // output is 12 raw bytes `0a00000200036d617001c000`, not gzip.
    let raw = std::fs::read(path).map_err(|error| fail(format!("{}: {error}", path.display())))?;
    let bytes = if raw.len() >= 2 && raw[0] == 0x1f && raw[1] == 0x8b {
        let mut decoder = flate2::read::GzDecoder::new(raw.as_slice()).take(64 * 1024 * 1024 + 1);
        let mut decoded = Vec::new();
        decoder
            .read_to_end(&mut decoded)
            .map_err(|error| fail(format!("{}: {error}", path.display())))?;
        decoded
    } else {
        raw
    };
    if bytes.len() > 64 * 1024 * 1024 {
        return Err(fail(format!("NBT exceeds budget: {}", path.display())));
    }
    let mut cursor = std::io::Cursor::new(bytes.as_slice());
    let (_, tag) = mc_nbt::read_named(&mut cursor)
        .map_err(|error| fail(format!("{}: {error}", path.display())))?;
    if cursor.position() != bytes.len() as u64 {
        return Err(fail(format!("trailing NBT: {}", path.display())));
    }
    Ok(tag)
}
pub(super) fn write_nbt(path: &Path, tag: &Tag) -> Result<(), CoreError> {
    fs::create_dir_all(
        path.parent()
            .ok_or_else(|| fail("NBT path has no parent"))?,
    )
    .map_err(fail)?;
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(fail)?;
    let mut encoder = flate2::write::GzEncoder::new(file, flate2::Compression::default());
    let mut bytes = Vec::new();
    mc_nbt::write_named(&mut bytes, "", tag).map_err(fail)?;
    encoder.write_all(&bytes).map_err(fail)?;
    encoder.finish().map_err(fail)?.sync_all().map_err(fail)
}
/// Read the declared version without treating opaque record fields as a schema.
/// No floor or ceiling: per-kind range gates live with the callers.
pub(super) fn declared_version(tag: &Tag, path: &Path) -> Result<i32, CoreError> {
    let Tag::Compound(root) = tag else {
        return Err(fail(format!("{}: expected compound", path.display())));
    };
    let declared = |fields: &[(String, Tag)]| -> Result<Option<i32>, CoreError> {
        let mut values = fields.iter().filter(|(key, _)| key == "DataVersion");
        let value = values.next().map(|(_, tag)| tag);
        if values.next().is_some() {
            return Err(fail(format!("{}: duplicate DataVersion", path.display())));
        }
        match value {
            Some(Tag::Int(version)) => Ok(Some(*version)),
            None => Ok(None),
            value => Err(fail(format!(
                "{}: invalid DataVersion {value:?}; expected integer",
                path.display()
            ))),
        }
    };
    let outer = declared(root)?;
    // Only level.dat's Data wrapper is typed. Any other record's opaque
    // Data field (e.g. a plugin payload's DataVersion) must never supply
    // the record version: a versionless player with opaque
    // Data{DataVersion:4790} must still report -1 and run the legacy
    // chain, not skip to current.
    let inner = if path.file_name().is_some_and(|name| name == "level.dat") {
        let mut wrappers = root.iter().filter(|(key, _)| key == "Data");
        let wrapper = wrappers.next().map(|(_, tag)| tag);
        if wrappers.next().is_some() {
            return Err(fail(format!("{}: duplicate Data wrapper", path.display())));
        }
        match wrapper {
            Some(Tag::Compound(fields)) => declared(fields)?,
            Some(_) => return Err(fail(format!("{}: invalid Data wrapper", path.display()))),
            None => None,
        }
    } else {
        None
    };
    if let (Some(outer), Some(inner)) = (outer, inner)
        && outer != inner
    {
        return Err(fail(format!(
            "{}: inconsistent DataVersion: root {outer}, Data {inner}",
            path.display()
        )));
    }
    inner
        .or(outer)
        .ok_or_else(|| fail(format!("{}: missing DataVersion", path.display())))
}

/// World-record source version: declared version gated to the region/level
/// floor (>= 2586) through the target.
pub(super) fn source_version(tag: &Tag, path: &Path) -> Result<i32, CoreError> {
    let version = declared_version(tag, path)?;
    if !(mc_world::anvil::upgrade::MIN_DATA_VERSION..=mc_world::anvil::TARGET_DATA_VERSION)
        .contains(&version)
    {
        return Err(fail(format!(
            "{}: {}",
            path.display(),
            mc_world::anvil::upgrade::UpgradeError::UnsupportedVersion(version)
        )));
    }
    Ok(version)
}

/// Player source version: declared version without the world floor.
/// Versionless records report -1 (vanilla `PlayerDataStorage` codec default,
/// proved in legacy-player-map-oracle.md); explicit stamps pass through
/// (the per-kind floor, players accept -1, lives in mc-world's upgrade
/// module). Stamps above target always fail closed.
pub(super) fn player_source_version(tag: &Tag, path: &Path) -> Result<i32, CoreError> {
    let version = match declared_version(tag, path) {
        Ok(version) => version,
        Err(error) if error.to_string().ends_with("missing DataVersion") => {
            return Ok(-1);
        }
        Err(error) => return Err(error),
    };
    if version > mc_world::anvil::TARGET_DATA_VERSION {
        return Err(fail(format!(
            "{}: {}",
            path.display(),
            mc_world::anvil::upgrade::UpgradeError::UnsupportedVersion(version)
        )));
    }
    Ok(version)
}

/// SavedDataStorage.readTagFromDisk uses 1343 for every absent saved-data
/// DataVersion, then dispatches its caller's DataFixTypes. Unlike chunks, saved
/// data has no 2586 floor. This only identifies the source envelope; callers
/// still either execute their specific upgrade or retain it explicitly deferred.
/// Official 26.1.2 bytecode: aux-dataversion-javap.txt:234-245.
pub(super) fn saved_data_source_version(tag: &Tag, path: &Path) -> Result<i32, CoreError> {
    let version = match declared_version(tag, path) {
        Ok(version) => version,
        Err(error) if error.to_string().ends_with("missing DataVersion") => 1343,
        Err(error) => return Err(error),
    };
    if version > mc_world::anvil::TARGET_DATA_VERSION {
        return Err(fail(format!(
            "{}: unsupported saved-data DataVersion {version}; newer than {}",
            path.display(),
            mc_world::anvil::TARGET_DATA_VERSION
        )));
    }
    Ok(version)
}

/// Vanilla 26.1.2 loads every saved-data file through DFU (`SavedDataStorage`
/// runs `DataFixTypes.update` to current on each read, then decodes with the
/// type codec; unchanged files are simply not rewritten on save, which is why
/// genuine `--forceUpgrade` output keeps files stamped 4772). DFU has no fixer
/// touching the world_clocks / wandering_trader / scheduled_events /
/// custom_boss_events payloads between 4772 and 4790 (the 4771 fixes only
/// rewrite level.dat fields; the 4772 entries are file moves, not NBT fixes),
/// so those envelopes decode identically at either version. Accept them at
/// their stored version instead of demanding an exact 4790 stamp; stay
/// fail-closed for newer-than-target stamps and for payloads a newer fixer
/// actually rewrites.
pub(super) fn check_version(tag: &Tag, path: &Path) -> Result<(), CoreError> {
    let version = source_version(tag, path)?;
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or_default();
    // Proven DFU-neutral across 4772..=4790: no registered NBT fixer reads or
    // writes these payloads, so the stored bytes decode exactly as 4790 would.
    // Evidence: .analysis/codex-logs/constantinople/aux-dataversion-javap.txt.
    const DFU_NEUTRAL_SINCE_4772: [&str; 4] = [
        "world_clocks.dat",
        "wandering_trader.dat",
        "scheduled_events.dat",
        "custom_boss_events.dat",
    ];
    if version == mc_world::anvil::TARGET_DATA_VERSION
        || (version == 4772 && DFU_NEUTRAL_SINCE_4772.contains(&name))
    {
        return Ok(());
    }
    if version > mc_world::anvil::TARGET_DATA_VERSION {
        return Err(fail(format!(
            "{}: unsupported auxiliary DataVersion {version}; newer than {}",
            path.display(),
            mc_world::anvil::TARGET_DATA_VERSION
        )));
    }
    Err(fail(format!(
        "{}: unsupported auxiliary DataVersion {version}; expected {}",
        path.display(),
        mc_world::anvil::TARGET_DATA_VERSION
    )))
}
fn sync_tree(root: &Path) -> Result<(), CoreError> {
    for path in files(root)? {
        File::open(root.join(path))
            .map_err(fail)?
            .sync_all()
            .map_err(fail)?;
    }
    fn directories(path: &Path) -> Result<(), CoreError> {
        for entry in fs::read_dir(path).map_err(fail)? {
            let entry = entry.map_err(fail)?;
            if entry.file_type().map_err(fail)?.is_dir() {
                directories(&entry.path())?;
            }
        }
        crate::journal::fsync_dir(path).map_err(fail)
    }
    directories(root)
}
fn write_receipt<T: Serialize>(path: &Path, receipt: &T) -> Result<(), CoreError> {
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent).map_err(fail)?;
    let temp = parent.join(format!(".conversion-receipt-{}", uuid::Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temp)
        .map_err(fail)?;
    serde_json::to_writer_pretty(&mut file, receipt).map_err(fail)?;
    file.write_all(b"\n").map_err(fail)?;
    file.sync_all().map_err(fail)?;
    fs::rename(&temp, path).map_err(fail)?;
    crate::journal::fsync_dir(parent).map_err(fail)
}
